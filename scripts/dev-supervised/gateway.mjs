import http from "node:http";

const CONTROL = "/_dev/supervised";
const safe = (method) => ["GET", "HEAD", "OPTIONS"].includes(method);

export function createGateway({ origin, status, backend, idle, rebuild }) {
    let commands = 0;
    const sockets = new Set();
    const busy = () =>
        ["checking", "building", "restarting"].includes(status.phase);
    const permittedHost = (request) =>
        request.headers.host === new URL(origin).host;
    const reply = (response, code, message, includeStatus = false) => {
        response.writeHead(code, {
            "Content-Type": "application/json",
            "Cache-Control": "no-store",
            "X-Content-Type-Options": "nosniff",
        });
        response.end(
            JSON.stringify(
                includeStatus ? { ...status, message } : { message },
            ),
        );
    };

    const server = http.createServer(async (request, response) => {
        if (!permittedHost(request))
            return reply(response, 403, "The host is not permitted.");
        if (request.url === CONTROL) {
            if (request.method === "GET")
                return reply(response, 200, status.message, true);
            if (request.method !== "POST")
                return reply(response, 405, "Use GET or POST.");
            if (
                request.headersDistinct.origin?.length !== 1 ||
                request.headers.origin !== origin ||
                request.headers["x-frinkworks-dev"] !== "restart"
            )
                return reply(response, 403, "The origin is not permitted.");
            if (busy() || commands !== 0) {
                return reply(
                    response,
                    409,
                    "Another request is active. Try again after it finishes.",
                    true,
                );
            }
            // Close command admission before the idle probe. No new work can race the restart.
            status.phase = "checking";
            status.message = "Idle check in progress.";
            try {
                const message = await idle();
                if (message) {
                    status.phase = "error";
                    status.message = message;
                    return reply(response, 409, message, true);
                }
                status.phase = "building";
                status.message = "Build in progress. Commands are unavailable.";
                reply(response, 202, status.message, true);
                void rebuild().catch((error) => {
                    status.phase = "error";
                    status.message = error.message;
                });
            } catch {
                status.phase = "error";
                status.message =
                    "The server did not answer the idle request. Try again.";
                reply(response, 503, status.message, true);
            }
            return;
        }
        // The application probe is private to the launcher, even on the public loopback URL.
        if (
            !request.url?.startsWith("/") ||
            request.url.startsWith("//") ||
            request.url.startsWith("/_dev/")
        ) {
            return reply(response, 404, "Not found.");
        }
        if (!safe(request.method) && busy()) {
            return reply(
                response,
                503,
                "A rebuild is active. Try again after the server restarts.",
            );
        }
        const port = backend();
        if (!port)
            return reply(
                response,
                503,
                "The server is unavailable. See the supervisor output.",
            );
        const command = !safe(request.method);
        if (command) commands++;
        const upstream = http.request({
            host: "127.0.0.1",
            port,
            path: request.url,
            method: request.method,
            headers: request.headers,
        });
        upstream.once("close", () => {
            if (command) commands--;
        });
        upstream.on("response", (result) => {
            response.writeHead(result.statusCode, result.headers);
            result.on("error", () => response.destroy());
            result.pipe(response);
        });
        upstream.on("error", () => {
            if (response.headersSent) response.destroy();
            else
                reply(
                    response,
                    503,
                    "The server is unavailable. See the supervisor output.",
                );
        });
        request.on("error", () => upstream.destroy());
        request.pipe(upstream);
    });

    server.on("upgrade", (request, socket, head) => {
        const port = backend();
        if (
            !permittedHost(request) ||
            request.headers.origin !== origin ||
            !port ||
            busy() ||
            request.url !== "/_hypergraft/live"
        ) {
            socket.destroy();
            return;
        }
        const upstream = http.request({
            host: "127.0.0.1",
            port,
            path: request.url,
            headers: request.headers,
        });
        sockets.add(socket);
        socket.on("error", () => socket.destroy());
        socket.on("close", () => {
            sockets.delete(socket);
            upstream.destroy();
        });
        upstream.on("error", () => socket.destroy());
        upstream.on("response", () => socket.destroy());
        upstream.on("upgrade", (response, peer, pending) => {
            peer.on("error", () => socket.destroy());
            socket.on("close", () => peer.destroy());
            const headers = response.rawHeaders;
            socket.write(
                `HTTP/1.1 101 Switching Protocols\r\n${Array.from(
                    { length: headers.length / 2 },
                    (_, index) =>
                        `${headers[index * 2]}: ${headers[index * 2 + 1]}\r\n`,
                ).join("")}\r\n`,
            );
            if (pending.length) socket.write(pending);
            if (head.length) peer.write(head);
            socket.pipe(peer).pipe(socket);
        });
        upstream.end();
    });
    return {
        server,
        disconnect: () => {
            for (const socket of sockets) socket.destroy();
        },
    };
}
