---
author: jl + claude
date: 2026-10-10
tags: [tooling]
status: final
---

# Lesson: `node --check` cannot parse a Workflow script

## What happened

`node --check .claude/workflows/audit-loop.js` fails with `SyntaxError: Illegal return statement` at the script's last line, yet the Workflow tool runs the script without trouble.

## Why it happened

A Workflow script is neither a module nor a CommonJS file. It is the body of an async function: it opens with `export const meta = {...}`, uses `await` at the top level, and ends with a top-level `return {...}`. Node parses it as a module because of the `export`, and a module cannot `return` at the top level. Without the `export`, Node parses it as CommonJS, and the first top-level `await` fails instead (`await is only valid in async functions and the top level bodies of modules`). Re-checked on 2026-10-11 with Node v26.8.2.

## What we learned

A `node --check` failure at the top-level `return` (or at a top-level `await`) is not a syntax error, and a pass proves nothing about how the Workflow tool reads the script. Node reports only the first error it finds, so a failure at an earlier line can still be a real one.

## How to avoid next time

- Parse the script as an async-function body, with the `export` keyword dropped from `meta`:

  ```bash
  node -e 'const AF = Object.getPrototypeOf(async function () {}).constructor;
  new AF(require("fs").readFileSync(process.argv[1], "utf8").replace(/^export /m, ""));
  console.log("ok")' .claude/workflows/audit-loop.js
  ```

  This only parses the script and does not run it. To check what it does, use a stubbed run that replaces the agent calls.
