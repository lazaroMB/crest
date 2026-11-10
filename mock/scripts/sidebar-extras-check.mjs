// Checks what the review added to the mock after the CREST-SIDEBAR spikes:
// archiving, branching a chat, and the shell command typed as `!`.
// Prints one line per check and exits 1 when any fails.
//
//   node scripts/sidebar-extras-check.mjs

import { commands, projects } from "../server/data.js";
import { groupOf, invocation, palette } from "../src/commands.js";
import { modeCommand } from "../src/modes.js";
import * as org from "../src/organization.js";

let failed = 0;
const check = (id, ok, detail = "") => {
  if (!ok) failed += 1;
  console.log(`${id} ${ok ? "met" : "not-met"}${detail ? `  (${detail})` : ""}`);
};

const base = org.addChat(org.fromDirectoryCatalog(projects, 1000), { id: "c-solo", title: "Solo", now: 2000 });
const ids = (list) => list.map((c) => c.id);

// Archive and restore.
{
  let o = org.archiveChat(base, "c-solo");
  const hidden = !ids(org.sidebarModel(o).standalone).includes("c-solo");
  const listed = ids(org.archivedModel(o).chats).includes("c-solo");
  o = org.unarchiveChat(o, "c-solo");
  check("archive.chat", hidden && listed && ids(org.sidebarModel(o).standalone).includes("c-solo"), "hidden, listed as archived, restored");

  let p = org.archiveProject(base, "p-crest");
  const gone = !org.sidebarModel(p).projects.some((x) => x.id === "p-crest");
  const chatsHidden = !org.sidebarModel(p).standalone.some((c) => c.projectId === "p-crest");
  const kept = org.archivedModel(p).projects.find((x) => x.id === "p-crest")?.chats.length === base.chats.filter((c) => c.projectId === "p-crest").length;
  p = org.unarchiveProject(p, "p-crest");
  check("archive.project", gone && chatsHidden && kept && org.sidebarModel(p).projects.some((x) => x.id === "p-crest"), "project and its chats hidden, kept, restored");

  check("archive.keeps-data", org.archiveChat(base, "c-solo").chats.length === base.chats.length, "archiving deletes nothing");
}

// Branch.
{
  const o = org.branchChat(base, "c-bugfix", "c-branch", 3000);
  const branch = o.chats.find((c) => c.id === "c-branch");
  const original = o.chats.find((c) => c.id === "c-bugfix");
  const before = base.chats.find((c) => c.id === "c-bugfix");
  check(
    "branch.chat",
    branch?.projectId === original.projectId && branch.branchedFrom?.chatId === "c-bugfix" && JSON.stringify(original) === JSON.stringify(before),
    `"${branch?.title}" in ${branch?.projectId}, original unchanged`,
  );
  const listed = org.sidebarModel(o).projects.find((p) => p.id === original.projectId).chats;
  check("branch.listed", ids(listed).includes("c-branch") && ids(listed).includes("c-bugfix"), "both chats in the project");
}

// The shell command is typed `!`, without a slash.
{
  const shell = commands.find((c) => c.usage.startsWith("!"));
  check("shell.usage", Boolean(shell) && !commands.some((c) => c.usage.startsWith("/!")), shell?.usage ?? "missing");
  check("shell.group", shell && invocation(shell.usage) === "!" && groupOf(shell)?.id === "chat", "shown as ! in Chat");
  const chatGroup = palette(commands, "/").rows.find((g) => g.id === "chat");
  check("shell.palette", chatGroup?.commands.some((c) => invocation(c.usage) === "!"), "listed under Chat");
  check("shell.mode", modeCommand("deep", "! cat ~/file") === "! cat ~/file", "a ! line is never wrapped in /research");
}

process.exit(failed ? 1 : 0);
