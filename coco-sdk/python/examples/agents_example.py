"""Example: registering custom agents at session-initialize time.

The wire field ``InitializeParams.agents`` is opaque
(``dict[str, dict[str, Any]]``) — coco-rs accepts whatever shape the
loaded agent definition expects. The Python SDK passes the dict
through untouched, so callers build the agent record themselves.
"""

import asyncio

from coco_sdk import CocoClient, NotificationMethod


async def main():
    researcher = {
        "agent_type": "researcher",
        "name": "Researcher",
        "description": "Research agent for reading code and searching.",
        "prompt": "You are a code researcher. Only read and search, never modify.",
        "tools": ["Read", "Glob", "Grep", "WebSearch"],
        "model_role": "explore",
        "max_turns": 5,
    }

    async with CocoClient(
        prompt="Use the researcher agent to find all TODO comments in the codebase",
        agents={"researcher": researcher},
    ) as client:
        async for event in client.events():
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                delta = event.as_agent_message_delta()
                if delta:
                    print(delta.delta, end="", flush=True)
            elif event.method == NotificationMethod.SUBAGENT_SPAWNED:
                spawned = event.as_subagent_spawned()
                if spawned:
                    print(f"\n[Agent spawned: {spawned.agent_type}]")
            elif event.method == NotificationMethod.SUBAGENT_COMPLETED:
                completed = event.as_subagent_completed()
                if completed:
                    print(f"\n[Agent completed: {completed.result[:100]}...]")
            elif event.method == NotificationMethod.TURN_COMPLETED:
                print()


if __name__ == "__main__":
    asyncio.run(main())
