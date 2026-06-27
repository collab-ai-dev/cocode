"""Example: TypedClient for structured output with Pydantic models."""

import asyncio

from pydantic import BaseModel

from coco_sdk import TypedClient


class CodeReview(BaseModel):
    """Structured code review output."""

    summary: str
    issues: list[str]
    score: int
    suggestions: list[str]


async def main():
    async with TypedClient(
        prompt="Review the main.rs file and provide a structured code review",
        output_type=CodeReview,
        max_turns=3,
    ) as client:
        # get_typed_result consumes events and returns the typed output
        review = await client.get_typed_result()
        print(f"Summary: {review.summary}")
        print(f"Score: {review.score}/10")
        print(f"Issues: {len(review.issues)}")
        for issue in review.issues:
            print(f"  - {issue}")

    # Or use get_typed_result_with_metadata for usage info too
    async with TypedClient(
        prompt="Review the lib.rs file",
        output_type=CodeReview,
    ) as client:
        review, metadata = await client.get_typed_result_with_metadata()
        print(f"\nScore: {review.score}/10")
        print(f"Tokens used: {metadata.usage.input_tokens + metadata.usage.output_tokens}")


if __name__ == "__main__":
    asyncio.run(main())
