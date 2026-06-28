import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import readline from "node:readline";

import { JSONDecodeError, ProcessError, TransportClosedError, CLIConnectionError } from "./errors.js";
import { findCocoBinary } from "./runtime.js";

export type JsonObject = Record<string, unknown>;

export interface Transport {
  start(): Promise<void>;
  sendLine(line: string): Promise<void>;
  nextRequestId(): number;
  readLines(): AsyncIterable<JsonObject>;
  close(): Promise<void>;
}

export type SubprocessTransportOptions = {
  binaryPath?: string | null;
  cwd?: string | null;
  env?: Record<string, string>;
  cliArgs?: string[];
  signal?: AbortSignal;
};

export class SubprocessCLITransport implements Transport {
  private readonly binaryPath: string;
  private readonly cwd?: string;
  private readonly envOverride?: Record<string, string>;
  private readonly cliArgs: string[];
  private readonly signal?: AbortSignal;
  private process: ChildProcessWithoutNullStreams | null = null;
  private requestCounter = 0;
  private stderrChunks: Buffer[] = [];
  private exitPromise: Promise<{ code: number | null; signal: NodeJS.Signals | null }> | null = null;

  constructor(options: SubprocessTransportOptions = {}) {
    this.binaryPath = findCocoBinary(options.binaryPath);
    this.cwd = options.cwd ?? undefined;
    this.envOverride = options.env;
    this.cliArgs = options.cliArgs ?? [];
    this.signal = options.signal;
  }

  async start(): Promise<void> {
    let lastError: unknown;
    for (let attempt = 0; attempt < 3; attempt += 1) {
      try {
        await this.startProcess();
        return;
      } catch (error) {
        lastError = error;
        await sleep(1000 * 2 ** attempt);
      }
    }
    throw new CLIConnectionError("Failed to start coco after 3 attempts", { cause: lastError });
  }

  private async startProcess(): Promise<void> {
    const env: NodeJS.ProcessEnv = { ...process.env, COCO_ENTRYPOINT: "sdk-ts", ...this.envOverride };
    const child = spawn(this.binaryPath, [...this.cliArgs, "sdk"], {
      cwd: this.cwd,
      env,
      stdio: ["pipe", "pipe", "pipe"],
      signal: this.signal,
    });

    child.stderr.on("data", (chunk: Buffer) => {
      this.stderrChunks.push(chunk);
    });
    this.process = child;
    this.exitPromise = new Promise((resolve) => {
      child.once("exit", (code, signal) => resolve({ code, signal }));
    });

    await new Promise<void>((resolve, reject) => {
      child.once("spawn", resolve);
      child.once("error", reject);
    });
  }

  async sendLine(line: string): Promise<void> {
    if (!this.process) throw new TransportClosedError("Transport not started");
    const payload = `${line.replace(/\n+$/, "")}\n`;
    await new Promise<void>((resolve, reject) => {
      this.process?.stdin.write(payload, (error) => {
        if (error) reject(error);
        else resolve();
      });
    });
  }

  nextRequestId(): number {
    this.requestCounter += 1;
    return this.requestCounter;
  }

  async *readLines(): AsyncIterable<JsonObject> {
    if (!this.process) throw new TransportClosedError("Transport not started");
    const rl = readline.createInterface({
      input: this.process.stdout,
      crlfDelay: Infinity,
    });

    try {
      for await (const line of rl) {
        if (!line.trim()) continue;
        try {
          const value = JSON.parse(line) as unknown;
          if (!value || typeof value !== "object" || Array.isArray(value)) {
            throw new JSONDecodeError("Expected JSON object from coco", line);
          }
          yield value as JsonObject;
        } catch (error) {
          if (error instanceof JSONDecodeError) throw error;
          throw new JSONDecodeError(`Malformed JSON from coco: ${(error as Error).message}`, line);
        }
      }

      const exit = await this.exitPromise;
      if (exit && (exit.code !== 0 || exit.signal)) {
        const detail = exit.signal ? `signal ${exit.signal}` : `code ${exit.code ?? 1}`;
        throw new ProcessError(`coco process exited with ${detail}`, {
          exitCode: exit.code,
          stderr: Buffer.concat(this.stderrChunks).toString("utf8"),
        });
      }
    } finally {
      rl.close();
    }
  }

  async close(): Promise<void> {
    const child = this.process;
    if (!child) return;
    this.process = null;

    child.stdin.end();
    if (!child.killed) child.kill();
    await Promise.race([this.exitPromise, sleep(5000)]);
    if (!child.killed) child.kill("SIGKILL");
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
