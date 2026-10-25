import { httpRouter, makeFunctionReference } from "convex/server";
import { httpAction } from "./_generated/server";

const http = httpRouter();

// A TS httpAction that runs an action in either runtime and returns its result.
for (const [path, target] of [
  ["/run-rust", "rust/spec:echo"],
  ["/run-node", "nodeSpec:echo"],
] as const) {
  http.route({
    path,
    method: "POST",
    handler: httpAction(async (ctx, req) => {
      const args = await req.json();
      const result = await ctx.runAction(makeFunctionReference<"action">(target), args);
      return new Response(JSON.stringify(result), {
        headers: { "content-type": "application/json" },
      });
    }),
  });
}

export default http;
