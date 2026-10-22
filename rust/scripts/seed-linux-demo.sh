#!/usr/bin/env bash
# Seeds a three-repo demo project for trying Pomelo by hand: api (Express + Postgres), web (Vite + React) and
# worker (a Node job runner on Redis). Each repo has some history, an `origin` that is a bare repo under
# ~/remotes, a gitignored .env (onboarding imports its values as secrets) and a feature branch on origin.
# Afterwards, onboard ~/code/myproject with "New project > Folders".
set -euo pipefail

CODE="${CODE:-$HOME/code/myproject}"
REMOTES="${REMOTES:-$HOME/remotes}"
if [ -e "$CODE" ] || [ -e "$REMOTES" ]; then
  echo "seed-linux-demo: $CODE or $REMOTES already exists; remove them to seed again" >&2
  exit 1
fi
mkdir -p "$CODE" "$REMOTES"

commit() {
  git add -A
  GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" git commit -qm "$2"
}

new_repo() {
  git init -q -b main "$CODE/$1"
  git -C "$CODE/$1" config user.name "Dev"
  git -C "$CODE/$1" config user.email "dev@example.com"
  git init -q --bare -b main "$REMOTES/$1.git"
  git -C "$CODE/$1" remote add origin "$REMOTES/$1.git"
  cd "$CODE/$1"
}

new_repo api
cat > package.json <<'JSON'
{
  "name": "api",
  "private": true,
  "type": "commonjs",
  "scripts": {
    "start": "node src/server.js",
    "migrate": "node scripts/migrate.js",
    "seed": "node scripts/seed.js"
  },
  "dependencies": {
    "express": "^4.21.2",
    "pg": "^8.13.1"
  }
}
JSON
printf 'node_modules/\n.env\nlog/\n' > .gitignore
cat > .env.example <<'ENV'
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/api_development
API_KEY=replace-me
ENV
printf 'DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/api_development\nAPI_KEY=demo-local-key\n' > .env
mkdir -p src scripts migrations
cat > src/server.js <<'JS'
const fs = require("fs");
const path = require("path");
const express = require("express");
const { Pool } = require("pg");

const pool = new Pool({ connectionString: process.env.DATABASE_URL });
const app = express();
fs.mkdirSync(path.join(__dirname, "..", "log"), { recursive: true });
const log = fs.createWriteStream(path.join(__dirname, "..", "log", "development.log"), { flags: "a" });

app.use((req, res, next) => {
  const started = Date.now();
  res.on("finish", () => {
    const line = `${new Date().toISOString()} ${req.method} ${req.url} ${res.statusCode} ${Date.now() - started}ms`;
    log.write(line + "\n");
    console.log(line);
  });
  next();
});

app.get("/health", (req, res) => res.json({ ok: true }));
app.get("/users", async (req, res) => {
  const { rows } = await pool.query("select id, email, name from users order by id");
  res.json(rows);
});
app.get("/orders", async (req, res) => {
  const { rows } = await pool.query("select id, user_id, total_cents, status from orders order by id");
  res.json(rows);
});

const port = Number(process.env.PORT || 3000);
app.listen(port, "127.0.0.1", () => console.log(`api listening on ${port}`));
setInterval(() => log.write(`${new Date().toISOString()} heartbeat\n`), 1000);
JS
cat > scripts/migrate.js <<'JS'
const fs = require("fs");
const path = require("path");
const { Client } = require("pg");

(async () => {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  await client.query("create table if not exists schema_migrations (name text primary key)");
  const dir = path.join(__dirname, "..", "migrations");
  for (const name of fs.readdirSync(dir).sort()) {
    const done = await client.query("select 1 from schema_migrations where name = $1", [name]);
    if (done.rowCount) continue;
    await client.query(fs.readFileSync(path.join(dir, name), "utf8"));
    await client.query("insert into schema_migrations (name) values ($1)", [name]);
    console.log(`migrated ${name}`);
  }
  await client.end();
})().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
JS
cat > migrations/001_create_users.sql <<'SQL'
create table users (
  id serial primary key,
  email text not null unique,
  name text not null,
  created_at timestamptz not null default now()
);
SQL
commit "2026-10-01T09:00:00" "Start the API with health and users"
cat > migrations/002_create_orders.sql <<'SQL'
create table orders (
  id serial primary key,
  user_id integer not null references users(id),
  total_cents integer not null,
  status text not null default 'pending',
  created_at timestamptz not null default now()
);
SQL
commit "2026-10-03T14:20:00" "Add orders"
cat > scripts/seed.js <<'JS'
const { Client } = require("pg");

(async () => {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  const people = [
    ["ada@example.com", "Ada"],
    ["grace@example.com", "Grace"],
    ["linus@example.com", "Linus"],
    ["margaret@example.com", "Margaret"],
    ["ken@example.com", "Ken"],
  ];
  for (const [email, name] of people) {
    await client.query("insert into users (email, name) values ($1, $2) on conflict (email) do nothing", [email, name]);
  }
  const statuses = ["pending", "paid", "shipped", "refunded"];
  for (let i = 0; i < 40; i++) {
    await client.query("insert into orders (user_id, total_cents, status) values ($1, $2, $3)", [
      (i % people.length) + 1,
      1000 + i * 137,
      statuses[i % statuses.length],
    ]);
  }
  await client.end();
  console.log("seeded users and orders");
})().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
JS
commit "2026-10-06T10:05:00" "Seed users and orders"
git push -q origin main
git checkout -q -b feat-login
cat > src/login.js <<'JS'
module.exports = function login(req, res) {
  res.status(501).json({ error: "not implemented yet" });
};
JS
commit "2026-10-08T16:40:00" "Sketch the login route"
git push -q origin feat-login
git checkout -q main

new_repo web
cat > package.json <<'JSON'
{
  "name": "web",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build"
  },
  "dependencies": {
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  },
  "devDependencies": {
    "@vitejs/plugin-react": "^4.3.4",
    "vite": "^6.0.7"
  }
}
JSON
printf 'node_modules/\ndist/\n.env.local\n' > .gitignore
printf 'VITE_API_URL=http://127.0.0.1:3000\n' > .env.local
cat > vite.config.js <<'JS'
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: { host: "127.0.0.1", port: Number(process.env.PORT || 5173), strictPort: true },
});
JS
cat > index.html <<'HTML'
<!doctype html>
<html lang="en">
  <head><meta charset="UTF-8" /><title>web</title></head>
  <body><div id="root"></div><script type="module" src="/src/main.jsx"></script></body>
</html>
HTML
mkdir -p src
cat > src/main.jsx <<'JSX'
import React from "react";
import { createRoot } from "react-dom/client";
import App from "./App.jsx";

createRoot(document.getElementById("root")).render(<App />);
JSX
cat > src/App.jsx <<'JSX'
export default function App() {
  return <h1>myproject</h1>;
}
JSX
commit "2026-10-02T11:00:00" "Start the web app"
cat > src/Users.jsx <<'JSX'
import { useEffect, useState } from "react";

export default function Users() {
  const [users, setUsers] = useState([]);
  useEffect(() => {
    fetch(`${import.meta.env.VITE_API_URL}/users`).then((r) => r.json()).then(setUsers);
  }, []);
  return <ul>{users.map((user) => <li key={user.id}>{user.name}</li>)}</ul>;
}
JSX
cat > src/App.jsx <<'JSX'
import Users from "./Users.jsx";

export default function App() {
  return (
    <main>
      <h1>myproject</h1>
      <Users />
    </main>
  );
}
JSX
commit "2026-10-05T15:30:00" "List users from the API"
git push -q origin main

new_repo worker
cat > package.json <<'JSON'
{
  "name": "worker",
  "private": true,
  "type": "commonjs",
  "scripts": { "start": "node src/worker.js" },
  "dependencies": { "redis": "^4.7.0" }
}
JSON
printf 'node_modules/\n.env\n' > .gitignore
printf 'REDIS_URL=redis://127.0.0.1:6379/0\nMAILER_TOKEN=demo-mailer-token\n' > .env
mkdir -p src
cat > src/worker.js <<'JS'
const { createClient } = require("redis");

(async () => {
  const redis = createClient({ url: process.env.REDIS_URL });
  await redis.connect();
  // A blocking pop holds its connection, so jobs are queued on a second one.
  const blocking = redis.duplicate();
  await blocking.connect();
  let next = 1;
  setInterval(async () => {
    await redis.lPush("jobs:email", JSON.stringify({ id: next, to: `user${next}@example.com` }));
    await redis.set(`job:${next}:status`, "queued");
    next += 1;
  }, 3000);
  for (;;) {
    const job = await blocking.brPop("jobs:email", 0);
    const { id } = JSON.parse(job.element);
    await redis.set(`job:${id}:status`, "sent");
    await redis.incr("stats:sent");
    console.log(`sent job ${id}`);
  }
})().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
JS
commit "2026-10-04T09:45:00" "Send queued emails from Redis"
git push -q origin main

echo "seeded $CODE (api, web, worker) with origins under $REMOTES"
