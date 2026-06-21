"""Convenience decorator for hooks.

The ``@hook()`` decorator generates a unique ``callback_id`` and bundles
it with the event/matcher/timeout into a :class:`HookDefinition`. When
a :class:`~coco_sdk.client.CocoClient` is constructed with
``hooks=[my_hook]``, it auto-registers the handler, transforms each
``HookDefinition`` into a wire-level
:class:`~coco_sdk.generated.protocol.HookCallbackMatcher` keyed by
event, and sends the resulting map in the ``initialize`` request::

    from coco_sdk import hook, CocoClient

    @hook(event="PreToolUse", matcher="Bash")
    async def block_dangerous(callback_id, event_type, input):
        if "rm -rf" in input.get("tool_input", {}).get("command", ""):
            return SdkHookOutput(
                hook_specific_output=HookSpecificOutput.PreToolUse(
                    permission_decision="deny",
                    permission_decision_reason="Blocked dangerous command",
                ),
            )
        return SdkHookOutput()

    async with CocoClient(prompt="...", hooks=[block_dangerous]) as client:
        async for event in client.events():
            print(event.method)

``HookDefinition`` is a Python-only construct — there is no wire-level
``HookCallbackConfig``. The wire shape is :class:`HookCallbackMatcher`
(generated from coco-rs); the decorator's metadata is collapsed into
that shape at initialize time.
"""

from __future__ import annotations

import uuid
from typing import Any, Awaitable, Callable

from coco_sdk.generated.protocol import SdkHookOutput

# Hook handlers may return either a raw dict (TS-canonical
# `hookJSONOutputSchema` shape, camelCase keys) or the typed
# :class:`SdkHookOutput` Pydantic model. The client normalizer dumps
# Pydantic models with `by_alias=True` so the wire stays camelCase.
HookOutput = dict[str, Any] | SdkHookOutput
HookFn = Callable[[str, str, dict[str, Any]], Awaitable[HookOutput]]


class HookDefinition:
    """A hook callback bound to its handler.

    Holds the SDK-side metadata (event, optional matcher regex,
    optional millisecond timeout) plus the Python coroutine that runs
    when the matching event fires. Each instance gets a unique
    ``callback_id`` so the client can route ``hook/callback`` server
    requests back to the right handler.
    """

    __slots__ = ("fn", "callback_id", "event", "matcher", "timeout_ms")

    def __init__(
        self,
        fn: HookFn,
        *,
        event: str,
        matcher: str | None = None,
        timeout_ms: int | None = None,
    ):
        self.fn = fn
        self.callback_id = uuid.uuid4().hex
        self.event = event
        self.matcher = matcher
        self.timeout_ms = timeout_ms

    async def __call__(
        self, callback_id: str, event_type: str, input: dict[str, Any]
    ) -> HookOutput:
        return await self.fn(callback_id, event_type, input)


def hook(
    *,
    event: str,
    matcher: str | None = None,
    timeout_ms: int | None = None,
) -> Callable[[HookFn], HookDefinition]:
    """Decorate an async function as a hook handler.

    Args:
        event: Hook event type (e.g. ``"PreToolUse"``, ``"PostToolUse"``).
        matcher: Tool-name regex (None matches all tools).
        timeout_ms: Per-callback timeout in milliseconds.
    """

    def decorator(fn: HookFn) -> HookDefinition:
        return HookDefinition(
            fn, event=event, matcher=matcher, timeout_ms=timeout_ms
        )

    return decorator
