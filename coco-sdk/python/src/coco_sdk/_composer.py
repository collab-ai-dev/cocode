"""Canonical turn builders for SDK-originated plain-text prompts."""

from coco_sdk.generated.protocol import (
    SessionTarget,
    SubmittedComposer,
    TurnStartRequest,
)


def build_plain_text_turn_start(target: SessionTarget, prompt: str) -> TurnStartRequest:
    """Build a turn with explicit empty atomic-composer metadata."""
    return TurnStartRequest(
        params=TurnStartRequest.TurnStartRequestParams(
            target=target,
            prompt=prompt,
            composer=SubmittedComposer(next_attachment_label=0),
        )
    )
