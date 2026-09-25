import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { once } from "node:events";
import { copyFile, mkdir, mkdtemp, rm } from "node:fs/promises";
import net from "node:net";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { createGateway } from "./gateway.mjs";

const root = path.resolve(import.meta.dirname, "../..");
const origin = process.env.FRINKWORKS_PUBLIC_ORIGIN ?? "http://localhost:4000";
const bind = new URL(
    `http://${process.env.FRINKWORKS_BIND_ADDRESS ?? "localhost:4000"}`,
);
const publicUrl = new URL(origin);
const loopback = (host) => ["localhost", "127.0.0.1", "[::1]"].includes(host);
if (
    !loopback(bind.hostname) ||
    !loopback(publicUrl.hostname) ||
    publicUrl.protocol !== "http:" ||
    publicUrl.origin !== origin ||
    process.env.FRINKWORKS_ENVIRONMENT !== "development"
)
    throw new Error(
        "Supervised development requires a loopback HTTP origin and the development environment.",
    );

await mkdir(path.join(root, ".cache"), { recursive: true });
const scratch = await mkdtemp(path.join(root, ".cache/dev-supervised-"));
const token = randomBytes(32).toString("hex");
const children = new Set();
const status = {
    phase: "building",
    revision: "",
    message: "Initial build in progress.",
};
let current;
let stopping = false;

function output(chunk) {
    process.stderr.write(chunk);
}

function launch(command, args, env = {}) {
    if (stopping) throw new Error("The supervisor stopped.");
    const child = spawn(command, args, {
        cwd: root,
        detached: true,
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, ...env },
    });
    children.add(child);
    child.done = new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("close", (code) => resolve(code));
    }).finally(() => children.delete(child));
    // Long-lived server failures also reach the supervisor status, not an unhandled rejection.
    void child.done.catch(() => {});
    child.stderr.on("data", output);
    return child;
}

function signal(child, name) {
    if (!child?.pid) return;
    try {
        process.kill(-child.pid, name);
    } catch (error) {
        if (error.code !== "ESRCH") throw error;
    }
}

async function stop(child) {
    if (!child) return;
    signal(child, "SIGTERM");
    const timeout = setTimeout(() => signal(child, "SIGKILL"), 3000);
    try {
        await child.done;
    } finally {
        clearTimeout(timeout);
    }
}

async function build() {
    const directory = await mkdtemp(path.join(scratch, "build-"));
    try {
        const assets = launch(
            "pnpm",
            [
                "vite",
                "build",
                "app",
                "--mode",
                "supervised",
                "--outDir",
                path.join(directory, "assets"),
            ],
            {
                VITE_SUPERVISED_REVISION: path.basename(directory),
            },
        );
        assets.stdout.on("data", output);
        if ((await assets.done) !== 0)
            throw new Error(
                "The asset build failed. The current server stays available.",
            );
        const rust = launch("cargo", [
            "build",
            "-p",
            "frinkworks",
            "--bin",
            "frinkworks",
            "--features",
            "dev",
            "--message-format=json-render-diagnostics",
        ]);
        let executable;
        const lines = createInterface({ input: rust.stdout });
        lines.on("line", (line) => {
            let message;
            try {
                message = JSON.parse(line);
            } catch {
                output(`${line}\n`);
                return;
            }
            if (
                message.reason === "compiler-artifact" &&
                message.target.name === "frinkworks" &&
                message.executable
            ) {
                executable = message.executable;
            }
        });
        if ((await rust.done) !== 0 || !executable)
            throw new Error(
                "The Rust build failed. The current server stays available.",
            );
        await copyFile(executable, path.join(directory, "frinkworks"));
        return directory;
    } catch (error) {
        await rm(directory, { recursive: true, force: true });
        throw error;
    }
}

async function probe(port) {
    return fetch(`http://127.0.0.1:${port}/_dev/supervised/idle`, {
        headers: { "x-frinkworks-supervisor": token },
        signal: AbortSignal.timeout(2000),
    });
}

async function start(directory) {
    const reservation = net.createServer();
    reservation.listen(0, "127.0.0.1");
    await once(reservation, "listening");
    const { port } = reservation.address();
    await new Promise((resolve) => reservation.close(resolve));
    const child = launch(path.join(directory, "frinkworks"), [], {
        FRINKWORKS_BIND_ADDRESS: `127.0.0.1:${port}`,
        FRINKWORKS_STATIC_DIR: path.join(directory, "assets"),
        FRINKWORKS_SUPERVISOR_TOKEN: token,
    });
    child.stdout.on("data", output);
    const instance = { child, port, directory };
    const deadline = Date.now() + 30000;
    while (Date.now() < deadline && !stopping) {
        if (!child.pid || child.exitCode !== null || child.signalCode !== null)
            break;
        try {
            const response = await probe(port);
            if ([204, 409].includes(response.status)) return instance;
        } catch {
            /* The socket does not exist until startup completes. */
        }
        await delay(100);
    }
    await stop(child);
    throw new Error(
        "The new server did not become ready. See the supervisor output.",
    );
}

async function rebuild() {
    let directory;
    try {
        directory = await build();
        if (stopping) return;
        status.phase = "restarting";
        status.message = "Server restart in progress.";
        gateway.disconnect();
        const previous = current;
        current = undefined;
        await stop(previous?.child);
        current = await start(directory);
        status.revision = path.basename(directory);
        status.phase = "ready";
        status.message = "Changes need a manual rebuild.";
        if (previous)
            await rm(previous.directory, {
                recursive: true,
                force: true,
            }).catch((error) => output(`${error.message}\n`));
        const instance = current;
        void instance.child.done.then(() => {
            if (!stopping && current === instance) {
                current = undefined;
                if (
                    !["checking", "building", "restarting"].includes(
                        status.phase,
                    )
                ) {
                    status.phase = "error";
                    status.message =
                        "The server exited. See the supervisor output, then rebuild.";
                }
            }
        });
    } catch (error) {
        status.phase = "error";
        status.message = error.message;
        output(`\n${error.message}\n`);
        if (directory && current?.directory !== directory)
            await rm(directory, { recursive: true, force: true });
    }
}

const gateway = createGateway({
    origin,
    status,
    backend: () => current?.port,
    idle: async () => {
        if (!current) return null;
        const response = await probe(current.port);
        if (response.status === 204) return null;
        if (response.status === 409) return response.text();
        throw new Error("The idle request failed.");
    },
    rebuild,
});

async function shutdown(exitCode = 0) {
    if (stopping) return;
    stopping = true;
    gateway.disconnect();
    gateway.server.close();
    gateway.server.closeAllConnections();
    await Promise.allSettled([...children].map(stop));
    await rm(scratch, { recursive: true, force: true });
    process.exit(exitCode);
}
process.once("SIGINT", () => void shutdown());
process.once("SIGTERM", () => void shutdown());
gateway.server.listen(
    Number(bind.port || 80),
    bind.hostname.replace(/^\[|\]$/g, ""),
);
try {
    await once(gateway.server, "listening");
    console.log(
        `Supervised development: ${origin}\nChanges require Rebuild and restart. Ctrl+C stops the supervisor.`,
    );
    await rebuild();
    if (!current) await shutdown(1);
} catch (error) {
    console.error(error.message);
    await shutdown(1);
}
