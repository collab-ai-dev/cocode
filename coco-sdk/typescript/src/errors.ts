export class CocoSDKError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = new.target.name;
  }
}

export class CLINotFoundError extends CocoSDKError {}

export class CLIConnectionError extends CocoSDKError {}

export class ProcessError extends CocoSDKError {
  readonly exitCode?: number | null;
  readonly stderr?: string;

  constructor(message: string, options: { exitCode?: number | null; stderr?: string; cause?: unknown } = {}) {
    super(message, { cause: options.cause });
    this.exitCode = options.exitCode;
    this.stderr = options.stderr;
  }
}

export class JSONDecodeError extends CocoSDKError {
  readonly rawLine?: string;

  constructor(message: string, rawLine?: string) {
    super(message);
    this.rawLine = rawLine;
  }
}

export class TransportClosedError extends CocoSDKError {}

export class SessionNotFoundError extends CocoSDKError {
  readonly sessionId: string;

  constructor(sessionId: string) {
    super(`Session not found: ${sessionId}`);
    this.sessionId = sessionId;
  }
}
