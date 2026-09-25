import assert from "node:assert/strict";
import { once } from "node:events";
import http from "node:http";
import { test } from "node:test";
import { createGateway } from "./gateway.mjs";

async function fixture(
    t,
    {
        idle = async () => null,
        rebuild = async () => {},
        handler = (_, response) => response.end("application"),
    } = {},
) {
    const app = http.createServer(handler);
    app.listen(0, "127.0.0.1");
    await once(app, "listening");
    const status = {
        phase: "ready",
        revision: "old",
        message: "Ready.",
    };
    // Use a fixed Host header independently of the ephemeral listen port.
    const origin = "http://localhost:4000";
    const gateway = createGateway({
        origin,
        status,
        backend: () => app.address().port,
        idle,
        rebuild,
    });
    gateway.server.listen(0, "127.0.0.1");
    await once(gateway.server, "listening");
    t.after(() => {
        gateway.disconnect();
        for (const server of [gateway.server, app]) {
            server.closeAllConnections();
            server.close();
        }
    });
    const request = (method = "GET", path = "/_dev/supervised", headers = {}) =>
        new Promise((resolve, reject) => {
            const outgoing = http.request(
                {
                    host: "127.0.0.1",
                    port: gateway.server.address().port,
                    path,
                    method,
                    headers: {
                        Host: "localhost:4000",
                        Origin: origin,
                        "X-Frinkworks-Dev": "restart",
                        ...headers,
                    },
                },
                (response) => {
                    let body = "";
                    response.on("data", (chunk) => {
                        body += chunk;
                    });
                    response.on("end", () =>
                        resolve({
                            status: response.statusCode,
                            headers: response.headers,
                            body,
                        }),
                    );
                },
            );
            outgoing.on("error", reject);
            outgoing.end();
        });
    return { request, status };
}

test("restart requires an exact origin, host and explicit browser header", async (t) => {
    let builds = 0;
    const { request, status } = await fixture(t, {
        rebuild: async () => {
            builds++;
        },
    });
    status.message = "Private supervisor status";
    for (const headers of [
        { Origin: "http://attacker.example" },
        { Origin: "null" },
        { Origin: "" },
        { Origin: ["http://localhost:4000", "http://localhost:4000"] },
        { Host: "attacker.example" },
        { "X-Frinkworks-Dev": "" },
    ]) {
        const response = await request("POST", "/_dev/supervised", headers);
        assert.equal(response.status, 403);
        assert.ok(!response.body.includes(status.message));
    }
    const rebound = await request("GET", "/_dev/supervised", {
        Host: "attacker.example",
    });
    assert.equal(rebound.status, 403);
    assert.ok(!rebound.body.includes(status.message));
    assert.equal(builds, 0);
    assert.equal((await request("GET")).headers["cache-control"], "no-store");
    assert.equal((await request("GET", "/_dev/supervised/idle")).status, 404);
    assert.equal((await request("POST")).status, 202);
    assert.equal(builds, 1);
});

test("the idle probe closes command admission and duplicate restart requests", async (t) => {
    let release;
    let entered;
    const probe = new Promise((resolve) => {
        entered = resolve;
    });
    const { request, status } = await fixture(t, {
        idle: () => {
            entered();
            return new Promise((resolve) => {
                release = resolve;
            });
        },
    });
    const restart = request("POST");
    await probe;
    assert.equal((await request("POST", "/conversations/new")).status, 503);
    assert.equal((await request("POST")).status, 409);
    assert.equal(
        (await request("GET", "/conversations/new")).body,
        "application",
    );
    release("Work is active.");
    assert.equal((await restart).status, 409);
    assert.equal(status.phase, "error");
    assert.equal((await request("POST", "/conversations/new")).status, 200);
});

test("an in-flight application command blocks the idle probe", async (t) => {
    let release;
    let entered;
    let probes = 0;
    const started = new Promise((resolve) => {
        entered = resolve;
    });
    const { request } = await fixture(t, {
        idle: async () => {
            probes++;
            return null;
        },
        handler: (_, response) => {
            release = () => response.end("done");
            entered();
        },
    });
    const command = request("POST", "/command");
    await started;
    assert.equal((await request("POST")).status, 409);
    assert.equal(probes, 0);
    release();
    await command;
});

test("an unavailable idle probe fails closed without a build", async (t) => {
    let builds = 0;
    const { request, status } = await fixture(t, {
        idle: async () => {
            throw new Error("offline");
        },
        rebuild: async () => {
            builds++;
        },
    });
    assert.equal((await request("POST")).status, 503);
    assert.equal(builds, 0);
    assert.equal(status.revision, "old");
});
