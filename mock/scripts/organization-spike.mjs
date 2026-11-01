// Runs the frozen checks of spikes S01, S02, S03 and criterion 1 of S04 and S06
// (CREST-SIDEBAR). Prints one line per criterion and a "k of N" per spike.
//
//   node scripts/organization-spike.mjs

import { commands, projects } from "../server/data.js";
import { palette, groupedCommands, groupOf } from "../src/commands.js";
import { modeCommand } from "../src/modes.js";
import * as org from "../src/organization.js";

const results = {};
const check = (spike, id, ok, detail = "") => {
  (results[spike] ??= []).push(ok);
  console.log(`${spike}.${id} ${ok ? "met" : "not-met"}${detail ? `  (${detail})` : ""}`);
};
const refused = (fn) => {
  try {
    fn();
    return null;
  } catch (error) {
    return error.message;
  }
};

const migrated = org.fromDirectoryCatalog(projects, 1000);

// S01: standalone chats and promotion.
{
  let o = org.addChat(migrated, { id: "c-new", now: 2000 });
  const before = org.sidebarModel(o);
  check("S01", 1, o.chats.find((c) => c.id === "c-new").projectId === null && before.standalone.some((c) => c.id === "c-new"));

  const blocks = [{ id: "u1", kind: "user", text: "hello" }];
  const store = { "c-new": { id: "c-new", blocks } };
  o = org.promoteChat(o, "c-new", "p-lab");
  const after = org.sidebarModel(o);
  const inLab = after.projects.find((p) => p.id === "p-lab").chats.some((c) => c.id === "c-new");
  check("S01", 2, inLab && !after.standalone.some((c) => c.id === "c-new") && store["c-new"].blocks === blocks, "id and blocks kept");

  let p = org.addChat(migrated, { id: "c-solo", now: 3000 });
  p = org.createProject(p, { id: "p-new", name: "Plot widget", roots: ["~/dev/plot"] });
  p = org.promoteChat(p, "c-solo", "p-new");
  const created = p.projects.find((x) => x.id === "p-new");
  check("S01", 3, created?.name === "Plot widget" && created.roots.length >= 1 && org.chatRoots(p, "c-solo")[0] === "~/dev/plot");

  const solo = org.addChat(migrated, { id: "c-alone" });
  check("S01", 4, org.chatRoots(solo, "c-alone").length === 0);
}

// S02: multi-directory projects and the migration.
{
  const ids = projects.flatMap((p) => p.chats.map((c) => c.id));
  check(
    "S02",
    1,
    migrated.projects.length === 3 && migrated.projects.every((p) => p.roots.length === 1) && ids.length === 9 && ids.every((id) => migrated.chats.some((c) => c.id === id)),
    `${migrated.projects.length} projects, ${migrated.chats.length} chats`,
  );

  let o = org.addRoot(migrated, "p-crest", "~/dev/api");
  o = org.addRoot(o, "p-crest", "/Volumes/data/models/");
  const three = o.projects.find((p) => p.id === "p-crest").roots.length;
  o = org.addRoot(o, "p-crest", "~/dev/api");
  const still = o.projects.find((p) => p.id === "p-crest").roots.length;
  check("S02", 2, three === 3 && still === 3, `${three} then ${still}`);

  const message = refused(() => org.removeRoot(migrated, "p-lab", "~/dev/lab-notes"));
  check("S02", 3, Boolean(message), message ?? "not refused");

  const roots = o.projects.find((p) => p.id === "p-crest").roots;
  const crestChats = o.chats.filter((c) => c.projectId === "p-crest");
  check("S02", 4, crestChats.length > 0 && crestChats.every((c) => org.chatRoots(o, c.id).join() === roots.join()));
}

// S03: grouped commands.
{
  const builtins = commands.filter((c) => !c.source);
  const groups = builtins.map(groupOf);
  check("S03", 1, builtins.length === 27 && groups.every((g) => g && g.id !== "other"), `${builtins.length} built-in, ${groups.filter((g) => !g).length} ungrouped`);

  const plugin = { usage: "/deploy <env>", description: "Deploy", source: "acme-deploy" };
  const pg = groupOf(plugin);
  check("S03", 2, pg.label.includes("acme-deploy") && groupedCommands([...commands, plugin]).some((g) => g.label === pg.label));

  const top = palette(commands, "/");
  const labels = groupedCommands(commands).map((g) => g.label);
  check("S03", 3, top.kind === "groups" && top.rows.length <= 8 && labels.every((l) => top.rows.some((r) => r.label === l)), `${top.rows.length} groups: ${labels.join(", ")}`);

  const res = palette(commands, "/res");
  const mode = palette(commands, "/mode plan");
  check(
    "S03",
    4,
    res.rows.length > 0 && res.rows.every((r) => r.usage.startsWith("/research") && r.group?.label) && mode?.rows.some((r) => r.usage.startsWith("/mode")),
    `${res.rows.length} research rows`,
  );
}

// S04 and S06, criterion 1: the line each mode sends.
{
  const ok =
    modeCommand("research", "why is add slow") === "/research why is add slow" &&
    modeCommand("deep", "why is add slow") === "/research --deep why is add slow" &&
    modeCommand("chat", "why is add slow") === "why is add slow";
  check("S06", 1, ok);
}

let failed = 0;
for (const [spike, oks] of Object.entries(results)) {
  const met = oks.filter(Boolean).length;
  console.log(`${spike}: ${met} of ${oks.length}`);
  if (met !== oks.length) failed += 1;
}
process.exit(failed ? 1 : 0);
