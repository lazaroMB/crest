// The mock backend, served by the Vite dev server under /api.
//
//   GET    /api/workbench           the organization (projects and chats), the open chat id and the status bar
//   GET    /api/commands            the / commands
//   GET    /api/chats/:id           one chat with its blocks
//   POST   /api/chats               { projectId | null } -> a new, empty chat (standalone when null)
//   PATCH  /api/chats/:id           { title?, projectId?, archived? } (projectId null takes it out of its project)
//   POST   /api/chats/:id/branch    a new chat that copies this chat's blocks up to now and diverges from there
//   DELETE /api/chats/:id
//   POST   /api/projects            { name, roots, chatId? } -> a new project, the chat promoted into it
//   PATCH  /api/projects/:id        { name?, addRoot?, removeRoot?, pinned?, archived? }
//   GET    /api/turn?chat=&prompt=  runs a scripted turn, streamed as server-sent events
//   POST   /api/approval/:id        { choice: once|session|project|all-edits|reject }
//   POST   /api/interrupt/:chatId
//   PATCH  /api/status              { mode?, model? }

import { chats, commands, projects, standalone, status } from "./data.js";
import { pickScenario } from "./scenarios.js";
import * as org from "../src/organization.js";

class Interrupted extends Error {}

const approvals = new Map(); // approval id -> resolve(choice)
const interrupts = new Map(); // chat id -> abort()
let openChatId = "c-bugfix";

// Today's store knows only directories; the organization starts as their migration.
const START = Date.now();
let organization = fromStore();

function fromStore() {
  const migrated = org.fromDirectoryCatalog(projects, START);
  return standalone.reduce(
    (o, chat, i) => org.renameChat(org.addChat(o, { id: chat.id, now: START - 2 - i }), chat.id, chat.title),
    migrated,
  );
}

/** Applies `change` to the organization; a refusal becomes a 409 with its message. */
function change(res, fn) {
  try {
    organization = fn(organization);
    send(res, { organization });
  } catch (error) {
    send(res, { error: error.message }, 409);
  }
}

function send(res, body, code = 200) {
  res.writeHead(code, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

async function readBody(req) {
  let text = "";
  for await (const chunk of req) text += chunk;
  return text ? JSON.parse(text) : {};
}

function setChat(chatId, patch) {
  organization = { ...organization, chats: organization.chats.map((c) => (c.id === chatId ? { ...c, ...patch } : c)) };
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

  const setActivity = (activity) => setChat(chat.id, { activity });

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
    setChat(chat.id, { title: chat.title });
    emit({ type: "chat", chatId: chat.id, title: chat.title });
  }

  setActivity("running");
  setChat(chat.id, { updatedAt: Date.now() });
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
  ["GET", /^\/api\/workbench$/, (req, res) => send(res, { organization, openChatId, status })],
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
      const { projectId = null } = await readBody(req);
      const id = `c-${Date.now().toString(36)}`;
      organization = org.addChat(organization, { id, projectId });
      chats[id] = { id, projectId, title: "New chat", blocks: [] };
      openChatId = id;
      send(res, chats[id]);
    },
  ],
  [
    "PATCH",
    /^\/api\/chats\/([\w-]+)$/,
    async (req, res, id) => {
      const body = await readBody(req);
      if (!chats[id]) return send(res, { error: "no such chat" }, 404);
      change(res, (o) => {
        let next = o;
        if (body.title !== undefined) next = org.renameChat(next, id, body.title);
        if (body.projectId !== undefined) next = org.moveChat(next, id, body.projectId);
        if (body.archived !== undefined) next = body.archived ? org.archiveChat(next, id) : org.unarchiveChat(next, id);
        if (body.title !== undefined) chats[id].title = body.title;
        if (body.projectId !== undefined) chats[id].projectId = body.projectId;
        if (body.archived !== undefined) chats[id].archived = body.archived;
        return next;
      });
    },
  ],
  [
    "POST",
    /^\/api\/chats\/([\w-]+)\/branch$/,
    (req, res, id) => {
      const source = chats[id];
      if (!source) return send(res, { error: "no such chat" }, 404);
      const newId = `c-${Date.now().toString(36)}`;
      const atBlock = source.blocks.length;
      organization = org.branchChat(organization, id, newId);
      const branched = organization.chats.find((c) => c.id === newId);
      if (branched) branched.branchedFrom.atBlock = atBlock;
      chats[newId] = {
        id: newId,
        projectId: source.projectId,
        title: branched?.title ?? `${source.title} (branch)`,
        branchedFrom: { chatId: id, title: source.title, atBlock },
        blocks: source.blocks.map((b) => ({ ...b })),
      };
      openChatId = newId;
      send(res, chats[newId]);
    },
  ],
  [
    "DELETE",
    /^\/api\/chats\/([\w-]+)$/,
    (req, res, id) => {
      delete chats[id];
      change(res, (o) => org.removeChat(o, id));
    },
  ],
  [
    "POST",
    /^\/api\/projects$/,
    async (req, res) => {
      const { name, roots, chatId } = await readBody(req);
      const id = `p-${Date.now().toString(36)}`;
      change(res, (o) => {
        let next = org.createProject(o, { id, name, roots });
        if (chatId) {
          next = org.moveChat(next, chatId, id);
          if (chats[chatId]) chats[chatId].projectId = id;
        }
        return next;
      });
    },
  ],
  [
    "PATCH",
    /^\/api\/projects\/([\w-]+)$/,
    async (req, res, id) => {
      const body = await readBody(req);
      change(res, (o) => {
        let next = o;
        if (body.name !== undefined) next = org.renameProject(next, id, body.name);
        if (body.addRoot) next = org.addRoot(next, id, body.addRoot);
        if (body.removeRoot) next = org.removeRoot(next, id, body.removeRoot);
        if (body.pinned !== undefined && Boolean(next.projects.find((p) => p.id === id)?.pinned) !== body.pinned) {
          next = org.togglePin(next, id);
        }
        if (body.archived !== undefined) next = body.archived ? org.archiveProject(next, id) : org.unarchiveProject(next, id);
        return next;
      });
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
            await handler(req, res, match[1]);
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
