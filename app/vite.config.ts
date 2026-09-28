/// <reference types="vitest/config" />
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig(({ mode }) => {
    const development = mode === "development";
    const suffix = development ? "" : "-[hash]";

    return {
        base: "/static/",
        plugins: [
            {
                name: "frinkworks-icons",
                buildStart() {
                    const script = fileURLToPath(
                        new URL(
                            "../scripts/build-icon-sprite.mjs",
                            import.meta.url,
                        ),
                    );
                    this.addWatchFile(script);
                    execFileSync(process.execPath, [script], {
                        stdio: "inherit",
                    });
                },
            },
            tailwindcss(),
        ],
        publicDir: "public",
        build: {
            outDir: development ? "static-development" : "static-production",
            emptyOutDir: true,
            manifest: true,
            rolldownOptions: {
                input: "assets/main.ts",
                output: {
                    entryFileNames: `assets/[name]${suffix}.js`,
                    chunkFileNames: `assets/[name]${suffix}.js`,
                    assetFileNames: `assets/[name]${suffix}[extname]`,
                },
            },
        },
        test: {
            include: ["app/assets/**/*.test.ts"],
        },
    };
});
