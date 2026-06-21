"""Multi-provider helper types for the Python SDK.

Most names here are **re-exports** of the schema-derived types in
``coco_sdk.generated.protocol`` — they mirror
``coco-rs/common/types/src/provider.rs`` (``ProviderApi``,
``ModelRole``, ``ModelSpec``, ``Capability``, ``WireApi``,
``ApplyPatchToolType``) and ``thinking.rs`` (``ThinkingLevel``,
``ReasoningEffort``). Re-exporting keeps the import path stable for
callers and centralizes the multi-provider story in one module.

Only **two** things are hand-written:

* :class:`ModelAlias` — defined in ``coco-rs/common/config``, not
  ``coco-types``. Aliases like ``"sonnet"`` / ``"opus"`` resolve to a
  different concrete model per provider; that lookup is config-layer
  business and intentionally not in the wire schema.
* :data:`DEEPSEEK` — a small bag of preset :class:`ModelSpec` instances
  matching ``coco-rs/common/config/src/provider/builtin.rs`` so e2e
  tests can refer to ``DEEPSEEK.flash_openai`` rather than literal
  strings.

Plus convenience helpers (:func:`thinking`, ``ModelSpec.cli_arg``)
that don't change wire shape.
"""

from __future__ import annotations

from enum import Enum
from typing import Any

from coco_sdk.generated.protocol import (
    ApplyPatchToolType,
    Capability,
    ModelRole,
    ModelSpec as _GeneratedModelSpec,
    PermissionMode,
    ProviderApi,
    ReasoningEffort,
    ThinkingLevel,
    WireApi,
)

__all__ = [
    "ApplyPatchToolType",
    "Capability",
    "DEEPSEEK",
    "ModelAlias",
    "ModelRole",
    "ModelSpec",
    "PermissionMode",
    "ProviderApi",
    "ReasoningEffort",
    "ThinkingLevel",
    "WireApi",
    "thinking",
]


class ModelSpec(_GeneratedModelSpec):
    """Generated ``ModelSpec`` + ergonomic CLI rendering.

    Adds :attr:`cli_arg` and ``__str__`` so callers can write
    ``CocoClient(models_main=spec)`` and the transport layer can ``str()`` it
    into the ``--models.main`` flag without a separate helper.
    """

    @property
    def cli_arg(self) -> str:
        """Render as ``"<provider>/<model_id>"`` for the CLI model arg."""
        return f"{self.provider}/{self.model_id}"

    def __str__(self) -> str:
        return self.cli_arg


class ModelAlias(str, Enum):
    """Provider-agnostic friendly names. Mirrors
    ``coco-rs/common/config/src/model/aliases.rs``.

    Hand-written because it lives in the config layer (not
    ``coco-types``) and resolves differently per provider — outside
    the wire schema.
    """

    SONNET = "sonnet"
    OPUS = "opus"
    HAIKU = "haiku"
    BEST = "best"
    SONNET_LARGE_CTX = "sonnet_large_ctx"
    OPUS_LARGE_CTX = "opus_large_ctx"
    OPUS_PLAN = "opus_plan"


def thinking(
    effort: ReasoningEffort | str = ReasoningEffort.medium,
    *,
    budget_tokens: int | None = None,
    options: dict[str, Any] | None = None,
) -> ThinkingLevel:
    """Build a :class:`ThinkingLevel` ergonomically.

    The unified reasoning model spans Anthropic ``thinking.budget_tokens``,
    OpenAI ``reasoning_effort``, Google ``thinking_config``, and
    DeepSeek reasoning content. ``options`` is provider-specific
    opaque passthrough.

    Example::

        from coco_sdk.types import ReasoningEffort, thinking

        level = thinking(ReasoningEffort.high, budget_tokens=8000)
    """
    if isinstance(effort, str) and not isinstance(effort, ReasoningEffort):
        effort = ReasoningEffort(effort)
    return ThinkingLevel(
        effort=effort,
        budget_tokens=budget_tokens,
        options=options,
    )


class _DeepSeekModels:
    """Named ``ModelSpec`` constants for DeepSeek's two builtin
    providers. Source of truth:
    ``coco-rs/common/config/src/provider/builtin.rs``.

    These are NOT defaults — they are explicit, named choices. Callers
    must pick one (e.g. ``DEEPSEEK.flash_openai``) rather than relying
    on a fallback. If the running coco binary doesn't have the named
    provider/model configured, the subprocess exits non-zero and
    :class:`coco_sdk.errors.ProcessError` is raised on the first
    ``session/start`` — there is no silent fallback.
    """

    flash_openai = ModelSpec(
        provider="deepseek-openai",
        model_id="deepseek-v4-flash",
        api=ProviderApi.openai_compat,
        display_name="DeepSeek V4 Flash",
    )
    flash_anthropic = ModelSpec(
        provider="deepseek-anthropic",
        model_id="deepseek-v4-flash",
        api=ProviderApi.anthropic,
        display_name="DeepSeek V4 Flash",
    )
    pro_openai = ModelSpec(
        provider="deepseek-openai",
        model_id="deepseek-v4-pro",
        api=ProviderApi.openai_compat,
        display_name="DeepSeek V4 Pro",
    )


DEEPSEEK = _DeepSeekModels()
