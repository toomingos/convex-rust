import { strict as assert } from "node:assert";
import { Prng } from "./prng.ts";

// Same vector as bench/convex/rust/src/prng.rs tests.
const p = new Prng(1);
assert.deepEqual([0, 1, 2, 3, 4].map(() => p.nextU32()), [1359504952,3827716927,3866437631,332804602,1758100174]);
console.log("prng ok");
