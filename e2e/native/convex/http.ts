import { httpRouter } from "convex/server";
import { rustHttpAction } from "./_generated/rustHttp";

const http = httpRouter();
for (const name of ["info", "echo", "double", "stream", "fail", "panics"]) {
  http.route({ path: `/${name}`, method: "POST", handler: rustHttpAction(`web:${name}`) });
}
http.route({ path: "/info", method: "GET", handler: rustHttpAction("web:info") });
export default http;
