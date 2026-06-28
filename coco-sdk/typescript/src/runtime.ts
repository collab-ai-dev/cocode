import { accessSync, constants } from "node:fs";
import { delimiter, join } from "node:path";

import { CLINotFoundError } from "./errors.js";

export type CocoRuntime = {
  binaryPath: string;
  source: "explicit" | "COCO_PATH" | "PATH" | "common_path";
};

export function resolveCocoRuntime(
  binaryPath?: string | null,
  env: NodeJS.ProcessEnv = process.env,
): CocoRuntime {
  if (binaryPath) return runtimeFromCandidate(binaryPath, "explicit");

  if (env.COCO_PATH) return runtimeFromCandidate(env.COCO_PATH, "COCO_PATH");

  const onPath = findOnPath("coco", env.PATH ?? "");
  if (onPath) return { binaryPath: onPath, source: "PATH" };

  for (const candidate of commonInstallPaths()) {
    if (isExecutable(candidate)) return { binaryPath: candidate, source: "common_path" };
  }

  throw new CLINotFoundError("coco binary not found. Install it or set COCO_PATH.");
}

export function findCocoBinary(binaryPath?: string | null, env: NodeJS.ProcessEnv = process.env): string {
  return resolveCocoRuntime(binaryPath, env).binaryPath;
}

function runtimeFromCandidate(binaryPath: string, source: CocoRuntime["source"]): CocoRuntime {
  if (!isExecutable(binaryPath)) {
    throw new CLINotFoundError(`coco binary not found at ${binaryPath}`);
  }
  return { binaryPath, source };
}

function findOnPath(name: string, pathValue: string): string | null {
  for (const dir of pathValue.split(delimiter)) {
    if (!dir) continue;
    const candidate = join(dir, name);
    if (isExecutable(candidate)) return candidate;
  }
  return null;
}

function commonInstallPaths(): string[] {
  const home = process.env.HOME;
  return [
    ...(home ? [join(home, ".cargo", "bin", "coco")] : []),
    "/usr/local/bin/coco",
  ];
}

function isExecutable(path: string): boolean {
  try {
    accessSync(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}
