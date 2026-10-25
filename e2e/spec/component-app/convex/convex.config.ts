import { defineApp } from "convex/server";
import inner from "../components/inner/convex.config";

const app = defineApp();
app.use(inner);
export default app;
