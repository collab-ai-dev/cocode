import {
  ClientRequestMethod,
  NotificationMethod,
  type PermissionMode,
  type ServerNotification,
} from "./generated/protocol.js";
import { MessageRouter } from "./messageRouter.js";
import { modelSpecToCliArg, type ModelSpec } from "./types.js";
import { SubprocessCLITransport } from "./transport.js";

export type QueryOptions = {
  modelsMain?: string | ModelSpec | null;
  maxTurns?: number | null;
  cwd?: string | null;
  appendSystemPrompt?: string | null;
  systemPrompt?: string | null;
  permissionMode?: PermissionMode | null;
  maxBudgetUsd?: number | null;
  env?: Record<string, string>;
  binaryPath?: string | null;
  signal?: AbortSignal;
};

export async function* query(prompt: string, options: QueryOptions = {}): AsyncGenerator<ServerNotification> {
  throwIfAborted(options.signal);
  const modelsMain = options.modelsMain ? modelSpecToCliArg(options.modelsMain) : undefined;
  const cliArgs: string[] = [];
  if (modelsMain) cliArgs.push("--models.main", modelsMain);

  const transport = new SubprocessCLITransport({
    binaryPath: options.binaryPath,
    cwd: options.cwd,
    env: options.env,
    cliArgs,
    signal: options.signal,
  });

  let router: MessageRouter | null = null;
  try {
    await transport.start();
    router = new MessageRouter(transport);
    router.start();
    await router.request({ method: ClientRequestMethod.INITIALIZE, params: {} });
    await router.request({
      method: ClientRequestMethod.SESSION_START,
      params: {
        model: modelsMain ?? null,
        max_turns: options.maxTurns ?? null,
        cwd: options.cwd ?? null,
        append_system_prompt: options.appendSystemPrompt ?? null,
        system_prompt: options.systemPrompt ?? null,
        permission_mode: options.permissionMode ?? null,
        max_budget_usd: options.maxBudgetUsd ?? null,
      },
    });
    await router.request({ method: ClientRequestMethod.TURN_START, params: { prompt } });

    while (true) {
      const event = (await router.nextEvent(options.signal)) as unknown as ServerNotification;
      throwIfAborted(options.signal);
      yield event;
      if (event.method === NotificationMethod.TURN_ENDED) break;
    }
  } finally {
    if (router) await router.close();
    else await transport.close();
  }
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    throw signal.reason instanceof Error ? signal.reason : new Error("Operation aborted");
  }
}
