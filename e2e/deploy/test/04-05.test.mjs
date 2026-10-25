import test, { after } from "node:test";
import assert from "node:assert/strict";
import { cleanup, makeApp, push, callRun, write, read, remove, constAction, clientCall } from "../lib/harness.mjs";

after(cleanup);

// normalised: prettier may or may not quote keys / wrap lines
const apiDts = (app) =>
  read(app, "convex/_generated/api.d.ts").replace(/"(\w+)":/g, "$1:").replace(/\s+/g, " ");

test("4. add / remove / rename a Rust action and a Rust module: codegen follows, removed fn is gone", async () => {
  const app = await makeApp();
  const multi = (...names) =>
    constAction(names[0], names[0]) +
    names.slice(1).map((n) => constAction(n, n).replace(/use convex_fn.*\nuse serde.*\n\n#\[derive\(Deserialize\)\]\npub struct NoArgs \{\}\n\n/, "")).join("\n");

  // v1: example.greet
  write(app, "convex/rust/src/example.rs", multi("greet"));
  let p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.match(apiDts(app), /greet: FunctionReference<"action", "public"/);

  // add an action
  write(app, "convex/rust/src/example.rs", multi("greet", "farewell"));
  p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.match(apiDts(app), /farewell: FunctionReference/);
  assert.match((await callRun(app, "rust/example:farewell", {})).stdout, /farewell/);

  // rename an action (farewell -> goodbye)
  write(app, "convex/rust/src/example.rs", multi("greet", "goodbye"));
  p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.match(apiDts(app), /goodbye: FunctionReference/);
  assert.doesNotMatch(apiDts(app), /farewell:/);
  assert.match((await callRun(app, "rust/example:goodbye", {})).stdout, /goodbye/);
  let gone = await callRun(app, "rust/example:farewell", {});
  assert.notEqual(gone.code, 0);
  assert.match(gone.out, /Could not find (public )?function/);

  // remove an action
  write(app, "convex/rust/src/example.rs", multi("greet"));
  p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.doesNotMatch(apiDts(app), /goodbye:/);
  gone = await callRun(app, "rust/example:goodbye", {});
  assert.match(gone.out, /Could not find (public )?function/);
  const c = await clientCall("action", "rust/example:goodbye", {});
  assert.match(JSON.stringify(c.body), /Could not find (public )?function/);

  // add a module, and a nested module
  write(app, "convex/rust/src/lib.rs", "pub mod example;\npub mod extra;\npub mod util;\n");
  write(app, "convex/rust/src/extra.rs", constAction("ping", "pong"));
  write(app, "convex/rust/src/util/mod.rs", "pub mod deep;\n");
  write(app, "convex/rust/src/util/deep.rs", constAction("dig", "deep!"));
  p = await push(app);
  assert.equal(p.code, 0, p.out);
  const d = apiDts(app);
  assert.match(d, /extra: \{[^}]*ping:/);
  assert.match(d, /util: \{[^}]*deep: \{[^}]*dig:/);
  assert.match((await callRun(app, "rust/extra:ping", {})).stdout, /pong/);
  assert.match((await callRun(app, "rust/util/deep:dig", {})).stdout, /deep!/);

  // rename a module (extra -> more)
  write(app, "convex/rust/src/lib.rs", "pub mod example;\npub mod more;\npub mod util;\n");
  write(app, "convex/rust/src/more.rs", constAction("ping", "pong"));
  remove(app, "convex/rust/src/extra.rs");
  p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.match(apiDts(app), /more: \{[^}]*ping:/);
  assert.doesNotMatch(apiDts(app), /extra:/);
  assert.match((await callRun(app, "rust/more:ping", {})).stdout, /pong/);
  assert.match((await callRun(app, "rust/extra:ping", {})).out, /Could not find (public )?function/);

  // remove modules entirely
  write(app, "convex/rust/src/lib.rs", "pub mod example;\n");
  remove(app, "convex/rust/src/more.rs");
  remove(app, "convex/rust/src/util");
  p = await push(app);
  assert.equal(p.code, 0, p.out);
  assert.doesNotMatch(apiDts(app), /more:|util:|dig:/);
  assert.match((await callRun(app, "rust/util/deep:dig", {})).out, /Could not find (public )?function/);
  assert.match((await callRun(app, "rust/more:ping", {})).out, /Could not find (public )?function/);
  assert.match((await callRun(app, "rust/example:greet", {})).stdout, /greet/);
});

test("5. #[action(internal)]: not callable from a client, callable via runAction from TS", async () => {
  const app = await makeApp();
  write(app, "convex/rust/src/lib.rs", "pub mod example;\npub mod secret;\n");
  write(app, "convex/rust/src/secret.rs", constAction("hidden", "top secret", "(internal)"));
  write(
    app,
    "convex/caller.ts",
    `import { action } from "./_generated/server";
import { internal } from "./_generated/api";

export const viaTs = action({
  args: {},
  handler: async (ctx): Promise<unknown> => {
    return await ctx.runAction(internal.rust.secret.hidden, {});
  },
});
`,
  );
  const p = await push(app);
  assert.equal(p.code, 0, p.out);
  // typed as internal in codegen
  assert.match(apiDts(app), /hidden: FunctionReference<"action", "internal"/);

  // external (unauthenticated) client: rejected
  const c = await clientCall("action", "rust/secret:hidden", {});
  assert.equal(c.body?.status, "error", JSON.stringify(c));
  assert.match(JSON.stringify(c.body), /Could not find public function/);

  // from TS via ctx.runAction(internal...)
  const r = await callRun(app, "caller:viaTs", {});
  assert.equal(r.code, 0, r.out);
  assert.match(r.stdout, /top secret/);

  // admin CLI can call internal functions directly
  const a = await callRun(app, "rust/secret:hidden", {});
  assert.equal(a.code, 0, a.out);
  assert.match(a.stdout, /top secret/);
});
