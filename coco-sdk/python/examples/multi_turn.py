"""Multi-turn session example."""

import asyncio

from coco_sdk import CocoClient, NotificationMethod


async def main():
    async with CocoClient(
        prompt="Create a hello world Python script",
        max_turns=3,
    ) as client:
        # First turn
        print("=== Turn 1 ===")
        async for event in client.events():
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                print(event.params.get("delta", ""), end="", flush=True)
            elif event.method == NotificationMethod.ITEM_COMPLETED:
                item = event.params.get("item", {})
                if item.get("type") == "file_change":
                    print(f"\n[File changed: {item}]")

        # Follow-up
        print("\n=== Turn 2 ===")
        async for event in client.send("Now add a docstring to the script"):
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                print(event.params.get("delta", ""), end="", flush=True)

        print("\nDone!")


if __name__ == "__main__":
    asyncio.run(main())
