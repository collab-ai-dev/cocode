import type { ModelSpec as GeneratedModelSpec, ReasoningEffort, ThinkingLevel } from "./generated/protocol.js";

export type ModelSpec = GeneratedModelSpec & {
  cliArg?: never;
};

export const ModelAlias = {
  SONNET: "sonnet",
  OPUS: "opus",
  HAIKU: "haiku",
  BEST: "best",
  SONNET_LARGE_CTX: "sonnet_large_ctx",
  OPUS_LARGE_CTX: "opus_large_ctx",
  OPUS_PLAN: "opus_plan",
} as const;

export type ModelAlias = (typeof ModelAlias)[keyof typeof ModelAlias];

export function modelSpecToCliArg(model: string | GeneratedModelSpec): string {
  if (typeof model === "string") return model;
  return `${model.provider}/${model.model_id}`;
}

export function thinking(
  effort: ReasoningEffort = "medium",
  options: { budgetTokens?: number; providerOptions?: Record<string, unknown> } = {},
): ThinkingLevel {
  return {
    effort,
    budget_tokens: options.budgetTokens ?? null,
    options: options.providerOptions ?? {},
  };
}

export const DEEPSEEK = {
  flashOpenai: {
    provider: "deepseek-openai",
    model_id: "deepseek-v4-flash",
    api: "openai_compat",
    display_name: "DeepSeek V4 Flash",
  } satisfies GeneratedModelSpec,
  flashAnthropic: {
    provider: "deepseek-anthropic",
    model_id: "deepseek-v4-flash",
    api: "anthropic",
    display_name: "DeepSeek V4 Flash",
  } satisfies GeneratedModelSpec,
  proOpenai: {
    provider: "deepseek-openai",
    model_id: "deepseek-v4-pro",
    api: "openai_compat",
    display_name: "DeepSeek V4 Pro",
  } satisfies GeneratedModelSpec,
} as const;
