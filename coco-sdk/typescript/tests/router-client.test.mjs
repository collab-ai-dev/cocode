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
const sessionStartResult = (session_id = "session-1") => ({ session_id });

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

test("router aborts an active server-request handler on cancellation", async () => {
  const transport = new MockTransport(
    [
      serverRequest("srv-cancel", ServerRequestMethod.INPUT_REQUEST_USER_INPUT, {
        request_id: "input-cancel",
      }),
      notification(ServerRequestMethod.CONTROL_CANCEL_REQUEST, {
        request_id: "srv-cancel",
      }),
    ],
    { keepOpen: true },
  );
  let aborted = false;
  const router = new MessageRouter(transport, async (_message, signal) => {
    await new Promise((resolve) => {
      signal.addEventListener("abort", () => {
        aborted = true;
        resolve();
      }, { once: true });
    });
    return true;
  });

  router.start();
  await eventually(() => aborted);
  assert.deepEqual(transport.sentLines, []);

  await router.close();
});

test("router swallows a handler rejection caused by cancellation", async () => {
  // A handler that honors the AbortSignal by rejecting (the losing side of
  // an approval broadcast) must not surface through nextEvent() and must
  // not write an error reply for the cancelled request.
  const transport = new MockTransport(
    [
      serverRequest("srv-abort", ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL, {
        request_id: "r-abort",
        tool_name: "Bash",
        input: {},
      }),
      notification(ServerRequestMethod.CONTROL_CANCEL_REQUEST, {
        request_id: "srv-abort",
      }),
      notification(NotificationMethod.TURN_ENDED, {
        turn_id: "t1",
        usage: {},
        outcome: { kind: "completed", data: { stop_reason: "end_turn" } },
      }),
    ],
    { keepOpen: true },
  );
  let rejected = false;
  const router = new MessageRouter(transport, async (_message, signal) => {
    await new Promise((_resolve, reject) => {
      signal.addEventListener(
        "abort",
        () => {
          rejected = true;
          reject(new Error("Operation aborted"));
        },
        { once: true },
      );
    });
    return true;
  });

  router.start();
  // If the rejection leaked, nextEvent() would throw before yielding.
  const event = await router.nextEvent();
  assert.equal(event.method, NotificationMethod.TURN_ENDED);
  assert.ok(rejected);
  assert.deepEqual(transport.sentLines, []);

  await router.close();
});

test("cancelRequest purges the pending correlation entry immediately", async () => {
  // A handler that never settles (and ignores its signal) must not leak
  // its correlation entry until close — the entry is dropped when the
  // cancellation arrives.
  const transport = new MockTransport(
    [
      serverRequest("srv-leak", ServerRequestMethod.INPUT_REQUEST_USER_INPUT, {
        request_id: "r-leak",
      }),
      notification(ServerRequestMethod.CONTROL_CANCEL_REQUEST, {
        request_id: "srv-leak",
      }),
    ],
    { keepOpen: true },
  );
  let sawRequest = false;
  const router = new MessageRouter(transport, async () => {
    sawRequest = true;
    await new Promise(() => {});
    return true;
  });

  router.start();
  await eventually(() => sawRequest);
  await eventually(() => router.serverRequestControllers.size === 0);
  assert.equal(router.serverRequestControllers.size, 0);
  assert.deepEqual(transport.sentLines, []);

  await router.close();
});

test("client withdraws via error reply when no canUseTool is configured", async () => {
  // Approvals are broadcast; a deny would consume the broadcast and cancel
  // a human peer's prompt. The handler-less client must reply with a
  // JSON-RPC error instead, which only withdraws this client.
  const transport = new MockTransport([
    response(1),
    response(2, sessionStartResult()),
    response(3),
    serverRequest("appr1", ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL, {
      request_id: "r1",
      tool_name: "Bash",
      tool_use_id: "tu1",
      input: {},
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
  const approvalReply = replies.find((line) => line.id === "appr1");
  assert.equal(approvalReply.result, undefined);
  assert.equal(approvalReply.error.code, -32601);
  assert.match(approvalReply.error.message, /no approval handler configured/);

  await client.close();
});

test("client passes the cancellation signal to canUseTool", async () => {
  const seen = [];
  const transport = new MockTransport([
    response(1),
    response(2, sessionStartResult()),
    response(3),
    serverRequest("appr2", ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL, {
      request_id: "r2",
      tool_name: "Read",
      input: { path: "/tmp/x" },
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
    canUseTool: async (toolName, input, signal) => {
      seen.push({ toolName, input, signal });
      return "allow";
    },
  });

  await client.start();
  for await (const _event of client.events()) {
    // drain to terminal event
  }

  assert.equal(seen.length, 1);
  assert.equal(seen[0].toolName, "Read");
  assert.deepEqual(seen[0].input, { path: "/tmp/x" });
  assert.ok(seen[0].signal instanceof AbortSignal);

  const replies = transport.sentLines.map((line) => JSON.parse(line));
  const approvalReply = replies.find((line) => line.id === "appr2");
  assert.deepEqual(approvalReply.result, { request_id: "r2", decision: "allow" });

  await client.close();
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
    response(2, sessionStartResult("session-close")),
    response(3),
    response(4),
  ]);
  const client = new CocoClient({ prompt: "hello", transport });

  await client.start();
  await client.closeSession();

  const requests = transport.sentLines.map((line) => JSON.parse(line));
  const closeRequest = requests.find((line) => line.method === ClientRequestMethod.SESSION_CLOSE);
  assert.deepEqual(closeRequest.params, {
    target: { session_id: "session-close" },
  });

  await assert.rejects(
    () => client.send("after close").next(),
    /no active session target/,
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
