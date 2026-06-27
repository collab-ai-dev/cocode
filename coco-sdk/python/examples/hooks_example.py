"""Example: Using @hook() to intercept tool execution."""

import asyncio

from coco_sdk import CocoClient, NotificationMethod, hook


@hook(event="PreToolUse", matcher="Bash")
async def block_dangerous_commands(callback_id, event_type, input_data):
    """Block dangerous shell commands."""
    command = input_data.get("tool_input", {}).get("command", "")
    if any(dangerous in command for dangerous in ["rm -rf", "dd if=", "mkfs"]):
        return {"behavior": "deny", "message": f"Blocked dangerous command: {command}"}
    return {"behavior": "allow"}


async def main():
    async with CocoClient(
        prompt="List all files in the current directory",
        hooks=[block_dangerous_commands.config],
    ) as client:
        # Register the hook handler
        client.on_hook(
            block_dangerous_commands.config.callback_id,
            block_dangerous_commands,
        )

        async for event in client.events():
            if event.method == NotificationMethod.AGENT_MESSAGE_DELTA:
                delta = event.as_agent_message_delta()
                if delta:
                    print(delta.delta, end="", flush=True)
            elif event.method == NotificationMethod.TURN_COMPLETED:
                print()


if __name__ == "__main__":
    asyncio.run(main())
