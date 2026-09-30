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
    for (const action of ["restart", "interrupt-restart"]) {
        for (const headers of [
            { Origin: "http://attacker.example" },
            { Origin: "null" },
            { Origin: "" },
            { Origin: ["http://localhost:4000", "http://localhost:4000"] },
            { Host: "attacker.example" },
            { "X-Frinkworks-Dev": "" },
            { "X-Frinkworks-Dev": "force" },
            { "X-Frinkworks-Dev": [action, action] },
        ]) {
            const response = await request("POST", "/_dev/supervised", {
                "X-Frinkworks-Dev": action,
                ...headers,
            });
            assert.equal(response.status, 403);
            assert.ok(!response.body.includes(status.message));
        }
    }
    const rebound = await request("GET", "/_dev/supervised", {
        Host: "attacker.example",
    });
    assert.equal(rebound.status, 403);
    assert.ok(!rebound.body.includes(status.message));
    assert.equal(builds, 0);
    assert.equal((await request("GET")).headers["cache-control"], "no-store");
    assert.equal((await request("GET", "/_dev/supervised/idle")).status, 404);
    const accepted = await request("POST");
    assert.equal(accepted.status, 202);
    assert.equal(JSON.parse(accepted.body).canInterrupt, false);
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
    const duplicate = await request("POST");
    assert.equal(duplicate.status, 409);
    assert.equal(JSON.parse(duplicate.body).canInterrupt, false);
    assert.equal(
        (await request("GET", "/conversations/new")).body,
        "application",
    );
    release("Work is active.");
    const blocked = await restart;
    assert.equal(blocked.status, 409);
    assert.equal(JSON.parse(blocked.body).canInterrupt, true);
    assert.equal(JSON.parse((await request("GET")).body).canInterrupt, false);
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
    const blocked = await request("POST");
    assert.equal(blocked.status, 409);
    assert.equal(JSON.parse(blocked.body).canInterrupt, true);
    assert.equal(probes, 0);
    release();
    await command;
});

test("explicit interruption bypasses active work and an unavailable idle probe", async (t) => {
    let builds = 0;
    let probes = 0;
    let unavailable = false;
    const { request } = await fixture(t, {
        idle: async () => {
            probes++;
            if (unavailable) throw new Error("offline");
            return "Work is active.";
        },
        rebuild: async () => {
            builds++;
        },
    });
    assert.equal((await request("POST")).status, 409);
    unavailable = true;
    assert.equal((await request("POST")).status, 503);
    assert.equal(builds, 0);
    assert.equal(
        (
            await request("POST", "/_dev/supervised", {
                "X-Frinkworks-Dev": "interrupt-restart",
            })
        ).status,
        202,
    );
    assert.equal(probes, 2);
    assert.equal(builds, 1);
    assert.equal((await request("POST", "/conversations/new")).status, 503);
    for (const action of ["restart", "interrupt-restart"]) {
        const duplicate = await request("POST", "/_dev/supervised", {
            "X-Frinkworks-Dev": action,
        });
        assert.equal(duplicate.status, 409);
        assert.equal(JSON.parse(duplicate.body).canInterrupt, false);
    }
    assert.equal(builds, 1);
});

test("explicit interruption bypasses an in-flight command but a failed build reopens admission", async (t) => {
    let release;
    let entered;
    let rejectBuild;
    const started = new Promise((resolve) => {
        entered = resolve;
    });
    const { request, status } = await fixture(t, {
        idle: async () => {
            assert.fail(
                "The interrupt action must not depend on the application.",
            );
        },
        handler: (request, response) => {
            if (request.url === "/blocked-command") {
                release = () => response.end("done");
                entered();
            } else response.end("application");
        },
        rebuild: () =>
            new Promise((_, reject) => {
                rejectBuild = reject;
            }),
    });
    const command = request("POST", "/blocked-command");
    await started;
    assert.equal((await request("POST")).status, 409);
    assert.equal(
        (
            await request("POST", "/_dev/supervised", {
                "X-Frinkworks-Dev": "interrupt-restart",
            })
        ).status,
        202,
    );
    assert.equal((await request("POST", "/conversations/new")).status, 503);
    rejectBuild(new Error("Build failed."));
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(status.phase, "error");
    assert.equal(status.revision, "old");
    assert.equal(JSON.parse((await request("GET")).body).canInterrupt, false);
    assert.equal(
        (await request("POST", "/conversations/new")).body,
        "application",
    );
    release();
    assert.equal((await command).body, "done");
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
    const unavailable = await request("POST");
    assert.equal(unavailable.status, 503);
    assert.equal(JSON.parse(unavailable.body).canInterrupt, true);
    assert.equal(builds, 0);
    assert.equal(status.revision, "old");
});
