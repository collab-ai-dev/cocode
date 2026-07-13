"""Structured-output helper tests."""

from __future__ import annotations

from typing import AsyncIterator

from pydantic import BaseModel

from coco_sdk.generated.protocol import (
    CompletedOutcome,
    ServerNotification,
    ServerNotificationTurnEnded,
    SessionResultParams,
    TokenUsage,
    TurnEndedParams,
    TurnOutcomeCompleted,
)
from coco_sdk.structured import TypedClient, _session_result_from_event


class StructuredAnswer(BaseModel):
    summary: str
    score: int


def _session_result(structured_output: object) -> SessionResultParams:
    return SessionResultParams(
        duration_api_ms=0,
        duration_ms=1,
        session_id="sess-structured",
        stop_reason="end_turn",
        total_cost_usd=0.0,
        total_turns=1,
        usage=TokenUsage(),
        structured_output=structured_output,
    )


def _turn_ended_with_result(result: SessionResultParams) -> ServerNotificationTurnEnded:
    return ServerNotificationTurnEnded(
        params=TurnEndedParams(
            turn_id="turn-structured-1",
            outcome=TurnOutcomeCompleted(data=CompletedOutcome(stop_reason=None)),
            session_result=result,
            usage=TokenUsage(),
        )
    )


def test_extracts_session_result_from_turn_ended() -> None:
    result = _session_result({"summary": "ok", "score": 9})

    assert _session_result_from_event(_turn_ended_with_result(result)) == result


async def test_typed_client_reads_structured_output_from_turn_ended() -> None:
    result = _session_result({"summary": "ok", "score": 9})
    event = _turn_ended_with_result(result)
    client = object.__new__(TypedClient)
    client._output_type = StructuredAnswer

    async def events() -> AsyncIterator[ServerNotification]:
        yield event

    client.events = events

    typed, metadata = await client.get_typed_result_with_metadata()

    assert typed == StructuredAnswer(summary="ok", score=9)
    assert metadata == result
