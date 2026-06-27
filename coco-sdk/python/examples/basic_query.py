"""Basic one-shot query example."""

import asyncio

from coco_sdk import NotificationMethod, query


async def main():
    async for event in query("What is 2 + 2?", max_turns=1):
        if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
            delta = event.params.get("delta", "")
            print(delta, end="", flush=True)
        elif event.method == NotificationMethod.TURN_COMPLETED:
            print("\n--- Turn completed ---")
        elif event.method == NotificationMethod.ERROR:
            print(f"\nError: {event.params.get('message', 'unknown')}")


if __name__ == "__main__":
    asyncio.run(main())
