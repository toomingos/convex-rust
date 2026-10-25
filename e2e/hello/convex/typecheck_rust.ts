import type { FunctionReference } from "convex/server";
import { api } from "./_generated/api";

const hello: FunctionReference<"action", "public"> = api.rust.hello.hello;
const fail: FunctionReference<"action", "public"> = api.rust.hello.fail;
export const refs = { hello, fail };
