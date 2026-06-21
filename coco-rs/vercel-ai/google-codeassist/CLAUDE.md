# vercel-ai-google-codeassist

Gemini **Code Assist** subscription transport (`cloudcode-pa.googleapis.com/v1internal`).
coco-original — there is **no** `@ai-sdk` Code Assist provider. Structured after
`@ai-sdk/google-vertex`'s reuse of `@ai-sdk/google`: depends on
`vercel-ai-google` and reuses its Gemini wire codec, swapping only the transport.

## What it reuses vs. owns

**Reused from `vercel-ai-google` (the codec):**
- `GoogleGenerativeAILanguageModel::get_args` — builds the inner `generateContent` body.
- `GoogleGenerativeAILanguageModel::map_response` — response → `GenerateResult`.
- `create_google_stream(.., ChunkEnvelope::CodeAssistWrapped)` — SSE framing + part state machine, with per-chunk `{response}` unwrap.
- `GoogleGenerateContentResponse`, `GoogleFailedResponseHandler`.

**Owned here (the transport):**
- `:method` RPC routing on `…/v1internal` (`:generateContent` / `:streamGenerateContent`).
- `Authorization: Bearer <oauth>` (vs. `x-goog-api-key`).
- Request envelope `{ model, project, user_prompt_id, request: <body> }` (`session_id` injected into `request`).
- Response unwrap `{ response: <body> }`.
- Lazy, cached project **onboarding** (`onboarding.rs`): `loadCodeAssist` → (if no tier) `onboardUser` → poll LRO → `project_id`. The one stateful piece (`Arc<Mutex<Option<OnboardingState>>>`) — the reason this is a separate crate rather than folded into the faithful `@ai-sdk/google` port.

## Key types

- `GoogleCodeAssistProvider` (`ProviderV4`) + `GoogleCodeAssistProviderSettings` + `create_google_code_assist`.
- `GoogleCodeAssistLanguageModel` (`LanguageModelV4`).
- `CodeAssistCreds { access_token, project_id }` + `CodeAssistCredsSupplier` (per-request Bearer supplier; mirrors `vercel_ai_openai::ChatGptCreds`).
- `code_assist_types`: the envelope + onboarding serde shapes (ported from jcode's `jcode-provider-gemini`).

## Conventions

- Coco-free (seam law); `coco-inference::model_factory::build_google` dispatches `ProviderAuth::OAuth { GeminiCodeAssist }` here via a neutral→wire `SubscriptionCreds → CodeAssistCreds` closure.
- Reads `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_PROJECT_ID` (vendor convention) as an onboarding shortcut.
- Errors are `vercel_ai_provider::AISdkError` (tier-2), like every provider crate.
- Wire contract locked by `tests/code_assist_wiremock.rs` (envelope, Bearer, unwrap, onboarding, SSE). The live Code Assist API is the source of truth; jcode (`agents/jcode`) is the reference impl.
