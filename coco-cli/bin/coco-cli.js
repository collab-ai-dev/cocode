#!/usr/bin/env node
// Unified npm launcher.

import { spawn } from "node:child_process";
import { existsSync } from "fs";
import { createRequire } from "node:module";
import path from "path";
import { fileURLToPath } from "url";

// __dirname equivalent in ESM
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);

const PRODUCT_NAME = "cocode";
const PACKAGE_SCOPE = "@cocode";
const PACKAGE_BASENAME = "cocode-cli";
const PACKAGE_NAME = `${PACKAGE_SCOPE}/${PACKAGE_BASENAME}`;

const PLATFORM_PACKAGE_BY_TARGET = {
  "x86_64-unknown-linux-musl": `${PACKAGE_NAME}-linux-x64`,
  "aarch64-unknown-linux-musl": `${PACKAGE_NAME}-linux-arm64`,
  "aarch64-apple-darwin": `${PACKAGE_NAME}-darwin-arm64`,
};

const { platform, arch } = process;

let targetTriple = null;
switch (platform) {
  case "linux":
  case "android":
    switch (arch) {
      case "x64":
        targetTriple = "x86_64-unknown-linux-musl";
        break;
      case "arm64":
        targetTriple = "aarch64-unknown-linux-musl";
        break;
      default:
        break;
    }
    break;
  case "darwin":
    if (arch === "arm64") {
      targetTriple = "aarch64-apple-darwin";
    }
    break;
  default:
    break;
}

if (!targetTriple) {
  throw new Error(`Unsupported platform: ${platform} (${arch})`);
}

const platformPackage = PLATFORM_PACKAGE_BY_TARGET[targetTriple];
if (!platformPackage) {
  throw new Error(`Unsupported target triple: ${targetTriple}`);
}

const cocoBinaryName = "coco";
const localVendorRoot = path.join(__dirname, "..", "vendor");
const localBinaryPath = path.join(
  localVendorRoot,
  targetTriple,
  "coco",
  cocoBinaryName,
);

let vendorRoot;
try {
  const packageJsonPath = require.resolve(`${platformPackage}/package.json`);
  vendorRoot = path.join(path.dirname(packageJsonPath), "vendor");
} catch {
  if (existsSync(localBinaryPath)) {
    vendorRoot = localVendorRoot;
  } else {
    throw new Error(missingDependencyMessage(platformPackage));
  }
}

if (!vendorRoot) {
  throw new Error(missingDependencyMessage(platformPackage));
}

const archRoot = path.join(vendorRoot, targetTriple);
const binaryPath = path.join(archRoot, "coco", cocoBinaryName);

// Use an asynchronous spawn instead of spawnSync so that Node is able to
// respond to signals (e.g. Ctrl-C / SIGINT) while the native binary is
// executing. This allows us to forward those signals to the child process
// and guarantees that when either the child terminates or the parent
// receives a fatal signal, both processes exit in a predictable manner.

/**
 * Use heuristics to detect the package manager that was used to install this package
 * in order to give the user a hint about how to update it.
 */
function detectPackageManager() {
  const userAgent = process.env.npm_config_user_agent || "";
  if (/\bbun\//.test(userAgent)) {
    return "bun";
  }

  const execPath = process.env.npm_execpath || "";
  if (execPath.includes("bun")) {
    return "bun";
  }

  if (
    __dirname.includes(".bun/install/global") ||
    __dirname.includes(".bun\\install\\global")
  ) {
    return "bun";
  }

  return userAgent ? "npm" : null;
}

function installCommand(packageManager) {
  return packageManager === "bun"
    ? `bun install -g ${PACKAGE_NAME}@latest`
    : `npm install -g ${PACKAGE_NAME}@latest`;
}

function missingDependencyMessage(platformPackage) {
  const packageManager = detectPackageManager();
  return `Missing optional dependency ${platformPackage}. Reinstall ${PRODUCT_NAME}: ${installCommand(packageManager)}`;
}

const env = { ...process.env };
const packageManagerEnvVar =
  detectPackageManager() === "bun"
    ? "COCO_MANAGED_BY_BUN"
    : "COCO_MANAGED_BY_NPM";
env[packageManagerEnvVar] = "1";

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  env,
});

child.on("error", (err) => {
  // Typically triggered when the binary is missing or not executable.
  // Re-throwing here will terminate the parent with a non-zero exit code
  // while still printing a helpful stack trace.
  // eslint-disable-next-line no-console
  console.error(err);
  process.exit(1);
});

// Forward common termination signals to the child so that it shuts down
// gracefully. In the handler we temporarily disable the default behavior of
// exiting immediately; once the child has been signaled we simply wait for
// its exit event which will in turn terminate the parent (see below).
const forwardSignal = (signal) => {
  if (child.killed) {
    return;
  }
  try {
    child.kill(signal);
  } catch {
    /* ignore */
  }
};

["SIGINT", "SIGTERM", "SIGHUP"].forEach((sig) => {
  process.on(sig, () => forwardSignal(sig));
});

// When the child exits, mirror its termination reason in the parent so that
// shell scripts and other tooling observe the correct exit status.
// Wrap the lifetime of the child process in a Promise so that we can await
// its termination in a structured way. The Promise resolves with an object
// describing how the child exited: either via exit code or due to a signal.
const childResult = await new Promise((resolve) => {
  child.on("exit", (code, signal) => {
    if (signal) {
      resolve({ type: "signal", signal });
    } else {
      resolve({ type: "code", exitCode: code ?? 1 });
    }
  });
});

if (childResult.type === "signal") {
  // Re-emit the same signal so that the parent terminates with the expected
  // semantics (this also sets the correct exit code of 128 + n).
  process.kill(process.pid, childResult.signal);
} else {
  process.exit(childResult.exitCode);
}
