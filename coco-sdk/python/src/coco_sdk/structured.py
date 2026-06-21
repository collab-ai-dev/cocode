"""Typed structured output helper.

Provides ``TypedClient`` — a generic wrapper that takes a Pydantic model,
passes its JSON schema through the ``initialize.json_schema`` field,
and deserializes the ``structured_output`` from ``session/result`` back
into the model::

    from pydantic import BaseModel
    from coco_sdk import TypedClient
    from coco_sdk.types import DEEPSEEK

    class CodeReview(BaseModel):
        summary: str
        issues: list[str]
        score: int

    async with TypedClient(prompt="Review main.rs",
                           output_type=CodeReview,
                           models_main=DEEPSEEK.flash_openai) as client:
        result = await client.get_typed_result()
        print(result.summary, result.score)
"""

from __future__ import annotations

from typing import Any, Generic, TypeVar

from pydantic import BaseModel

from coco_sdk.client import CocoClient
from coco_sdk.generated.protocol import SessionResultParams

T = TypeVar("T", bound=BaseModel)


class TypedClient(CocoClient, Generic[T]):
    """CocoClient that produces typed structured output."""

    def __init__(
        self,
        prompt: str,
        *,
        output_type: type[T],
        **kwargs: Any,
    ):
        self._output_type = output_type
        if "json_schema" in kwargs:
            schema = kwargs.pop("json_schema")
        else:
            schema = output_type.model_json_schema()
        super().__init__(prompt, json_schema=schema, **kwargs)

    async def get_typed_result(self) -> T:
        """Consume events and return the typed structured output.

        Raises ``ValueError`` if no structured output is returned.
        """
        result, _ = await self.get_typed_result_with_metadata()
        return result

    async def get_typed_result_with_metadata(self) -> tuple[T, SessionResultParams]:
        """Return both typed output and the raw session-result metadata."""
        session_result: SessionResultParams | None = None
        async for event in self.events():
            sr = event.as_session_result()
            if sr:
                session_result = sr

        if session_result is None or session_result.structured_output is None:
            raise ValueError("No structured output returned from session")

        try:
            typed = self._output_type.model_validate(session_result.structured_output)
        except Exception as exc:
            raise ValueError(
                f"Structured output does not match {self._output_type.__name__}: {exc}"
            ) from exc

        return typed, session_result
