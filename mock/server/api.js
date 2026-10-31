// The mock backend, served by the Vite dev server under /api.
//
//   GET    /api/workbench           projects, the open chat id and the status bar
//   GET    /api/commands            the / commands
//   GET    /api/chats/:id           one chat with its blocks
//   POST   /api/chats               { projectId } -> a new, empty chat
//   PATCH  /api/chats/:id           { title }
//   DELETE /api/chats/:id
//   GET    /api/turn?chat=&prompt=  runs a scripted turn, streamed as server-sent events
//   POST   /api/approval/:id        { choice: once|session|project|all-edits|reject }
//   POST   /api/interrupt/:chatId
//   PATCH  /api/status              { mode?, model? }
//   /api/mcp…                       the MCP servers (server/mcp.js)

import { chats, commands, projects, status } from "./data.js";
import { mcpRoutes } from "./mcp.js";
import { pickScenario } from "./scenarios.js";

class Interrupted extends Error {}

const approvals = new Map(); // approval id -> resolve(choice)
const interrupts = new Map(); // chat id -> abort()
let openChatId = "c-bugfix";

function send(res, body, code = 200) {
  res.writeHead(code, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

async function readBody(req) {
  let text = "";
  for await (const chunk of req) text += chunk;
  return text ? JSON.parse(text) : {};
}

function chatSummary(chatId) {
  return projects.flatMap((p) => p.chats).find((c) => c.id === chatId);
}

function upsert(chat, block) {
  const at = chat.blocks.findIndex((b) => b.id === block.id);
  if (at === -1) chat.blocks.push(block);
  else chat.blocks[at] = block;
}

async function runTurn(req, res) {
  const url = new URL(req.url, "http://mock");
  const chat = chats[url.searchParams.get("chat")];
  const prompt = url.searchParams.get("prompt") ?? "";
  if (!chat) return send(res, { error: "no such chat" }, 404);

  res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" });
  const emit = (event) => res.write(`data: ${JSON.stringify(event)}\n\n`);

  let abort;
  const aborted = new Promise((_, reject) => (abort = () => reject(new Interrupted())));
  aborted.catch(() => {});
  interrupts.set(chat.id, abort);
  req.on("close", abort);

  const summary = chatSummary(chat.id);
  const setActivity = (activity) => summary && (summary.activity = activity);

  const turn = {
    push(block) {
      upsert(chat, block);
      emit({ type: "block", block });
    },
    activity(verb, detail, stage) {
      const activity = verb ? { verb, detail, stage, startedAt: Date.now() } : null;
      emit({ type: "activity", activity });
    },
    notice(level, text) {
      emit({ type: "notice", notice: { level, text } });
    },
    setStatus(patch) {
      Object.assign(status, patch);
      emit({ type: "status", status: patch });
    },
    spend(dollars) {
      turn.setStatus({ cost: status.cost + dollars });
    },
    sleep(ms) {
      return Promise.race([new Promise((r) => setTimeout(r, ms)), aborted]);
    },
    approve(request) {
      setActivity("waiting");
      emit({ type: "approval", request });
      const answer = new Promise((resolve) => approvals.set(request.id, resolve));
      return Promise.race([answer, aborted]).finally(() => {
        approvals.delete(request.id);
        setActivity("running");
        emit({ type: "approval", request: null });
      });
    },
  };

  if (chat.title === "New chat" && prompt) {
    chat.title = prompt.length > 40 ? `${prompt.slice(0, 39).trimEnd()}…` : prompt;
    if (summary) summary.title = chat.title;
    emit({ type: "chat", chatId: chat.id, title: chat.title });
  }

  setActivity("running");
  turn.push({ id: `user-${Date.now().toString(36)}`, kind: "user", text: prompt });
  try {
    await pickScenario(prompt)(turn);
  } catch (error) {
    if (!(error instanceof Interrupted)) throw error;
    turn.notice("info", "turn interrupted");
  } finally {
    interrupts.delete(chat.id);
    setActivity(undefined);
    emit({ type: "activity", activity: null });
    emit({ type: "done" });
    res.end();
  }
}

const routes = [
  ["GET", /^\/api\/workbench$/, (req, res) => send(res, { projects, openChatId, status })],
  ["GET", /^\/api\/commands$/, (req, res) => send(res, commands)],
  [
    "GET",
    /^\/api\/chats\/([\w-]+)$/,
    (req, res, id) => {
      if (!chats[id]) return send(res, { error: "no such chat" }, 404);
      openChatId = id;
      send(res, chats[id]);
    },
  ],
  [
    "POST",
    /^\/api\/chats$/,
    async (req, res) => {
      const { projectId } = await readBody(req);
      const project = projects.find((p) => p.id === projectId) ?? projects[0];
      const id = `c-${Date.now().toString(36)}`;
      chats[id] = { id, projectId: project.id, title: "New chat", blocks: [] };
      project.chats.unshift({ id, title: "New chat" });
      openChatId = id;
      send(res, chats[id]);
    },
  ],
  [
    "PATCH",
    /^\/api\/chats\/([\w-]+)$/,
    async (req, res, id) => {
      const { title } = await readBody(req);
      if (!chats[id]) return send(res, { error: "no such chat" }, 404);
      chats[id].title = title;
      chatSummary(id).title = title;
      send(res, chats[id]);
    },
  ],
  [
    "DELETE",
    /^\/api\/chats\/([\w-]+)$/,
    (req, res, id) => {
      delete chats[id];
      for (const project of projects) project.chats = project.chats.filter((c) => c.id !== id);
      send(res, { ok: true });
    },
  ],
  ["GET", /^\/api\/turn$/, runTurn],
  [
    "POST",
    /^\/api\/approval\/([\w-]+)$/,
    async (req, res, id) => {
      const { choice } = await readBody(req);
      approvals.get(id)?.(choice);
      send(res, { ok: true });
    },
  ],
  [
    "POST",
    /^\/api\/interrupt\/([\w-]+)$/,
    (req, res, id) => {
      interrupts.get(id)?.();
      send(res, { ok: true });
    },
  ],
  [
    "PATCH",
    /^\/api\/status$/,
    async (req, res) => {
      Object.assign(status, await readBody(req));
      send(res, status);
    },
  ],
  ...mcpRoutes({ send, readBody }),
];

export function mockApi() {
  return {
    name: "crest-mock-api",
    configureServer(server) {
      server.middlewares.use(async (req, res, next) => {
        const path = req.url.split("?")[0];
        for (const [method, pattern, handler] of routes) {
          const match = path.match(pattern);
          if (!match || req.method !== method) continue;
          try {
            await handler(req, res, ...match.slice(1));
          } catch (error) {
            server.config.logger.error(String(error?.stack ?? error));
            if (!res.headersSent) send(res, { error: String(error) }, 500);
          }
          return;
        }
        next();
      });
    },
  };
}
