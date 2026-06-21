"""In-process tool support via the @tool() decorator.

Tools are registered as local MCP servers routed through the SDK's
``mcp/routeMessage`` control channel.

Example::

    from coco_sdk import tool, CocoClient

    @tool()
    def get_weather(city: str) -> str:
        \"\"\"Get current weather for a city.\"\"\"
        return f"Sunny in {city}"

    async with CocoClient(prompt="What is the weather?", tools=[get_weather]) as client:
        async for event in client.events():
            print(event.method)
"""

from __future__ import annotations

import inspect
import json
import logging
import uuid
from typing import Any, Callable, get_type_hints

logger = logging.getLogger(__name__)

# Sentinel type to identify decorated tools
class ToolDefinition:
    """A decorated tool function with MCP metadata."""

    def __init__(
        self,
        fn: Callable[..., Any],
        *,
        name: str | None = None,
        description: str | None = None,
    ):
        self.fn = fn
        self.name = name or fn.__name__
        self.description = description or (fn.__doc__ or "").strip().split("\n")[0]
        self.server_name = f"sdk_tool_{self.name}_{uuid.uuid4().hex[:8]}"
        self._schema = _build_input_schema(fn)

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        return self.fn(*args, **kwargs)

    def to_mcp_tool_def(self) -> dict[str, Any]:
        """Return the MCP tool definition dict."""
        return {
            "name": self.name,
            "description": self.description,
            "inputSchema": self._schema,
        }

    async def invoke(self, arguments: dict[str, Any]) -> Any:
        """Invoke the underlying function with the given arguments."""
        result = self.fn(**arguments)
        if inspect.isawaitable(result):
            result = await result
        return result

    def to_sdk_mcp_config(self) -> tuple[str, dict[str, Any]]:
        """Return (server_name, config_dict) for McpServerConfig::Sdk."""
        return (
            self.server_name,
            {
                "Sdk": {
                    "tools": [
                        {
                            "name": self.name,
                            "description": self.description,
                            "input_schema": self._schema,
                        }
                    ]
                }
            },
        )


def tool(
    *, name: str | None = None, description: str | None = None
) -> Callable[[Callable[..., Any]], ToolDefinition]:
    """Decorator to register a function as an in-process MCP tool.

    Args:
        name: Tool name (defaults to function name).
        description: Tool description (defaults to first docstring line).
    """

    def decorator(fn: Callable[..., Any]) -> ToolDefinition:
        return ToolDefinition(fn, name=name, description=description)

    return decorator


def _build_input_schema(fn: Callable[..., Any]) -> dict[str, Any]:
    """Build a JSON Schema for the function's parameters."""
    sig = inspect.signature(fn)
    try:
        hints = get_type_hints(fn)
    except Exception as exc:
        logger.warning("Failed to get type hints for %s: %s", fn.__name__, exc)
        hints = {}

    properties: dict[str, Any] = {}
    required: list[str] = []

    for param_name, param in sig.parameters.items():
        if param_name in ("self", "cls"):
            continue

        hint = hints.get(param_name)
        schema_type = _python_type_to_json_schema(hint)

        if isinstance(schema_type, dict):
            prop = schema_type
        else:
            prop = {"type": schema_type}

        if param.default is inspect.Parameter.empty:
            required.append(param_name)

        properties[param_name] = prop

    schema: dict[str, Any] = {"type": "object", "properties": properties}
    if required:
        schema["required"] = required
    return schema


def _python_type_to_json_schema(hint: Any) -> dict[str, Any] | str:
    """Map a Python type hint to a JSON Schema type or schema dict."""
    import typing

    if hint is None:
        return "string"
    if hint is str:
        return "string"
    if hint is int:
        return "integer"
    if hint is float:
        return "number"
    if hint is bool:
        return "boolean"

    # Get origin for generic types (list, dict, Optional, etc.)
    origin = getattr(hint, "__origin__", None)

    # list[T] / List[T]
    if origin is list:
        args = getattr(hint, "__args__", ())
        items = _python_type_to_json_schema(args[0]) if args else "string"
        if isinstance(items, str):
            return {"type": "array", "items": {"type": items}}
        return {"type": "array", "items": items}

    # dict[K, V] / Dict[K, V]
    if origin is dict:
        args = getattr(hint, "__args__", ())
        if len(args) >= 2:
            val_schema = _python_type_to_json_schema(args[1])
            if isinstance(val_schema, str):
                return {"type": "object", "additionalProperties": {"type": val_schema}}
            return {"type": "object", "additionalProperties": val_schema}
        return {"type": "object"}

    # Optional[T] / T | None (typing.Union with None)
    if origin is typing.Union:
        args = [a for a in hint.__args__ if a is not type(None)]
        if len(args) == 1:
            return _python_type_to_json_schema(args[0])
        # Multi-type union
        schemas = []
        for a in args:
            s = _python_type_to_json_schema(a)
            schemas.append({"type": s} if isinstance(s, str) else s)
        return {"anyOf": schemas}

    # Pydantic BaseModel subclass
    try:
        from pydantic import BaseModel as PydanticBase

        if isinstance(hint, type) and issubclass(hint, PydanticBase):
            return hint.model_json_schema()
    except ImportError:
        pass

    return "string"
