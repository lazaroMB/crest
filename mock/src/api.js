// The mock server's API (server/api.js).

async function call(method, url, body) {
  const res = await fetch(url, {
    method,
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const reply = await res.json();
  if (!res.ok) throw new Error(reply.error ?? `${method} ${url}: ${res.status}`);
  return reply;
}

export const api = {
  workbench: () => call("GET", "/api/workbench"),
  commands: () => call("GET", "/api/commands"),
  chat: (id) => call("GET", `/api/chats/${id}`),
  newChat: (projectId = null) => call("POST", "/api/chats", { projectId }),
  renameChat: (id, title) => call("PATCH", `/api/chats/${id}`, { title }),
  moveChat: (id, projectId) => call("PATCH", `/api/chats/${id}`, { projectId }),
  patchChat: (id, patch) => call("PATCH", `/api/chats/${id}`, patch),
  branchChat: (id) => call("POST", `/api/chats/${id}/branch`),
  deleteChat: (id) => call("DELETE", `/api/chats/${id}`),
  createProject: (body) => call("POST", "/api/projects", body),
  patchProject: (id, patch) => call("PATCH", `/api/projects/${id}`, patch),
  approve: (id, choice) => call("POST", `/api/approval/${id}`, { choice }),
  interrupt: (chatId) => call("POST", `/api/interrupt/${chatId}`),
  setStatus: (patch) => call("PATCH", "/api/status", patch),

  /** Runs a turn; `onEvent` gets every event until `{ type: "done" }`. */
  turn(chatId, prompt, onEvent) {
    const query = new URLSearchParams({ chat: chatId, prompt });
    const source = new EventSource(`/api/turn?${query}`);
    source.onmessage = (message) => {
      const event = JSON.parse(message.data);
      if (event.type === "done") source.close();
      onEvent(event);
    };
    source.onerror = () => {
      source.close();
      onEvent({ type: "done" });
    };
  },
};
