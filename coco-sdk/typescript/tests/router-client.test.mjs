import assert from "node:assert/strict";
import test from "node:test";

import { CocoClient, ClientRequestMethod, NotificationMethod, ServerRequestMethod } from "../dist/index.js";
import { MessageRouter } from "../dist/messageRouter.js";

class MockTransport {
  constructor(responses = [], options = {}) {
    this.responses = responses;
    this.keepOpen = options.keepOpen ?? false;
    this.sentLines = [];
    this.started = false;
    this.closed = false;
    this.requestId = 0;
  }

  async start() {
    this.started = true;
  }

  async sendLine(line) {
    this.sentLines.push(line);
  }

  nextRequestId() {
    this.requestId += 1;
    return this.requestId;
  }

  async *readLines() {
    for (const response of this.responses) {
      await Promise.resolve();
      yield response;
    }
    while (this.keepOpen && !this.closed) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  }

  async close() {
    this.closed = true;
  }
}

const response = (id, result = {}) => ({ jsonrpc: "2.0", id, result });
const notification = (method, params = {}) => ({ jsonrpc: "2.0", method, params });
const serverRequest = (id, method, params = {}) => ({ jsonrpc: "2.0", id, method, params });
const sessionStartResult = (session_id = "session-1", surface_id = "surface-1") => ({
  session_id,
  surface_id,
});

test("router replies with JSON-RPC error for unhandled server requests", async () => {
  const transport = new MockTransport([
    serverRequest("srv1", ServerRequestMethod.INPUT_REQUEST_USER_INPUT, { request_id: "r1" }),
  ]);
  const router = new MessageRouter(transport);

  router.start();
  await eventually(() => transport.sentLines.length === 1);

  const sent = JSON.parse(transport.sentLines[0]);
  assert.equal(sent.id, "srv1");
  assert.equal(sent.error.code, -32601);
  assert.match(sent.error.message, /unsupported server request/);

  await router.close();
});

test("client handles hook callbacks without leaking them as notifications", async () => {
  const transport = new MockTransport([
    response(1),
    response(2, sessionStartResult()),
    response(3),
    serverRequest("hook1", ServerRequestMethod.HOOK_CALLBACK, {
      callback_id: "cb1",
      event_type: "PreToolUse",
      input: { hook_event_name: "PreToolUse", tool_name: "Read", tool_input: {} },
    }),
    notification(NotificationMethod.TURN_ENDED, {
      turn_id: "t1",
      usage: {},
      outcome: { kind: "completed", data: { stop_reason: "end_turn" } },
    }),
  ]);
  const client = new CocoClient({
    prompt: "hello",
    transport,
    hookHandlers: {
      cb1: async () => ({ continue: true }),
    },
  });

  await client.start();
  const events = [];
  for await (const event of client.events()) events.push(event);

  const replies = transport.sentLines.map((line) => JSON.parse(line));
  const hookReply = replies.find((line) => line.id === "hook1");
  assert.deepEqual(hookReply.result, { output: { continue: true } });
  assert.deepEqual(events.map((event) => event.method), [NotificationMethod.TURN_ENDED]);

  await client.close();
});

test("client returns nested JSON-RPC error for unregistered SDK MCP server", async () => {
  const transport = new MockTransport([
    response(1),
    response(2, sessionStartResult()),
    response(3),
    serverRequest("mcp1", ServerRequestMethod.MCP_ROUTE_MESSAGE, {
      server_name: "missing",
      message: { jsonrpc: "2.0", id: 99, method: "tools/list" },
    }),
    notification(NotificationMethod.TURN_ENDED, {
      turn_id: "t1",
      usage: {},
      outcome: { kind: "completed", data: { stop_reason: "end_turn" } },
    }),
  ]);
  const client = new CocoClient({ prompt: "hello", transport });

  await client.start();
  for await (const _event of client.events()) {
    // drain to terminal event
  }

  const replies = transport.sentLines.map((line) => JSON.parse(line));
  const mcpReply = replies.find((line) => line.id === "mcp1");
  assert.equal(mcpReply.result.message.id, 99);
  assert.equal(mcpReply.result.message.error.code, -32601);

  await client.close();
});

test("send observes an already-aborted signal before writing turn/start", async () => {
  const transport = new MockTransport([], { keepOpen: true });
  const client = new CocoClient({ prompt: "hello", transport });
  const controller = new AbortController();
  controller.abort(new Error("stop"));

  await assert.rejects(async () => {
    for await (const _event of client.send("next", { signal: controller.signal })) {
      // no events expected
    }
  }, /stop/);
  assert.equal(transport.sentLines.length, 0);
});

test("send interrupts an in-flight turn when its signal aborts", async () => {
  const transport = new MockTransport(
    [
      response(1),
      response(2, sessionStartResult()),
      response(3),
      response(4),
    ],
    { keepOpen: true },
  );
  const client = new CocoClient({ prompt: "hello", transport });
  const controller = new AbortController();

  await client.start();
  const sendPromise = (async () => {
    for await (const _event of client.send("next", { signal: controller.signal })) {
      // no events expected
    }
  })();

  await eventually(() =>
    transport.sentLines.some((line) => JSON.parse(line).method === ClientRequestMethod.TURN_START && JSON.parse(line).id === 4),
  );
  controller.abort(new Error("stop"));

  await assert.rejects(sendPromise, /stop/);
  const methods = transport.sentLines.map((line) => JSON.parse(line).method);
  assert.equal(methods.at(-1), ClientRequestMethod.TURN_INTERRUPT);

  await client.close();
});

test("client sends target-aware close for active session and clears target", async () => {
  const transport = new MockTransport([
    response(1),
    response(2, sessionStartResult("session-close", "surface-close")),
    response(3),
    response(4),
  ]);
  const client = new CocoClient({ prompt: "hello", transport });

  await client.start();
  await client.closeSession();

  const requests = transport.sentLines.map((line) => JSON.parse(line));
  const closeRequest = requests.find((line) => line.method === ClientRequestMethod.SESSION_CLOSE);
  assert.deepEqual(closeRequest.params, {
    target: {
      kind: "interactive",
      target: {
        session_id: "session-close",
        surface_id: "surface-close",
      },
    },
  });

  await assert.rejects(
    () => client.send("after close").next(),
    /no active interactive session target/,
  );
  await client.close();
});

test("client sends storage-only delete request", async () => {
  const transport = new MockTransport([
    response(1),
    response(2, sessionStartResult()),
    response(3),
    response(4),
  ]);
  const client = new CocoClient({ prompt: "hello", transport });

  await client.start();
  await client.deleteSession("session-delete");

  const requests = transport.sentLines.map((line) => JSON.parse(line));
  const deleteRequest = requests.find((line) => line.method === ClientRequestMethod.SESSION_DELETE);
  assert.deepEqual(deleteRequest.params, { target: { session_id: "session-delete" } });

  await client.close();
});

async function eventually(predicate) {
  const deadline = Date.now() + 500;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.ok(predicate());
}
