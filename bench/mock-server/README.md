Start: `cd bench && MOCK_PORT=8787 node mock-server/server.ts` (clients read `MOCK_URL`, default `http://127.0.0.1:8787`).
Endpoints: `GET /bytes/:n?seed=` (PRNG bytes), `GET /stream/:n?seed=` (same bytes, chunked), `POST /echo` ({length, sha256} of body), `GET /sleep/:ms`.
Also `GET /redirect/:k` (k 302 hops, then 200 `{done, hops}`) and `GET /json/:n?seed=` (n objects `{i,name,score}`).
