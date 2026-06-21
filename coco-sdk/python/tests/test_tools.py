"""Tests for the @tool() decorator."""

import asyncio

from coco_sdk.tools import ToolDefinition, tool


def test_tool_decorator_creates_definition():
    @tool()
    def greet(name: str) -> str:
        """Say hello."""
        return f"Hello, {name}"

    assert isinstance(greet, ToolDefinition)
    assert greet.name == "greet"
    assert greet.description == "Say hello."


def test_tool_decorator_custom_name():
    @tool(name="my_greeter", description="Custom greet")
    def greet(name: str) -> str:
        return f"Hi, {name}"

    assert greet.name == "my_greeter"
    assert greet.description == "Custom greet"


def test_tool_input_schema():
    @tool()
    def add(a: int, b: int) -> int:
        """Add two numbers."""
        return a + b

    schema = add._schema
    assert schema["type"] == "object"
    assert "a" in schema["properties"]
    assert "b" in schema["properties"]
    assert schema["properties"]["a"]["type"] == "integer"
    assert "a" in schema["required"]
    assert "b" in schema["required"]


def test_tool_optional_param():
    @tool()
    def search(query: str, limit: int = 10) -> str:
        return query

    schema = search._schema
    assert "query" in schema["required"]
    assert "limit" not in schema["required"]


def test_tool_to_mcp_tool_def():
    @tool()
    def echo(text: str) -> str:
        """Echo back."""
        return text

    mcp_def = echo.to_mcp_tool_def()
    assert mcp_def["name"] == "echo"
    assert mcp_def["description"] == "Echo back."
    assert "inputSchema" in mcp_def


def test_tool_to_sdk_mcp_config():
    @tool()
    def echo(text: str) -> str:
        """Echo."""
        return text

    server_name, config = echo.to_sdk_mcp_config()
    assert server_name.startswith("sdk_tool_echo_")
    assert "Sdk" in config
    assert len(config["Sdk"]["tools"]) == 1
    assert config["Sdk"]["tools"][0]["name"] == "echo"


def test_tool_invoke_sync():
    @tool()
    def multiply(a: int, b: int) -> int:
        """Multiply."""
        return a * b

    result = asyncio.run(multiply.invoke({"a": 3, "b": 4}))
    assert result == 12


def test_tool_invoke_async():
    @tool()
    async def async_add(a: int, b: int) -> int:
        """Async add."""
        return a + b

    result = asyncio.run(async_add.invoke({"a": 5, "b": 7}))
    assert result == 12


def test_tool_callable():
    @tool()
    def double(x: int) -> int:
        """Double."""
        return x * 2

    assert double(5) == 10
