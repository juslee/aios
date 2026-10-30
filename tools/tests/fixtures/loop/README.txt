Fixtures for the R2a loop stage runner (tools/src/cmd/loop).

Recorded 2026-10-27 with claude 2.1.311 (Claude Code) and gh version 2.103.3 (2026-10-26).
Real outputs, sanitised: absolute paths replaced with <ROOT>, email addresses with
user@example.invalid. Logins (juslee, copilot-pull-request-reviewer, Copilot) are kept.

claude/ (claude -p --output-format json, structured output via --json-schema)
  success.json         full default toolset call, sonnet. The event-array form (system init,
                       assistant, rate_limit_event, user, result). The init event's mcp_servers
                       is replaced with [] and its plugins with the three built-in names
                       (cc-plugin-agents-md, cc-plugin-telemetry, cc-plugin-plugin-authoring)
                       to keep the file small.
  fable.json           small-form call with --model fable: the single result-object form.
  denied.json          the result event of a --permission-mode dontAsk call with a narrow
                       --allowedTools: two Bash writes denied (permission_denials), one allowed
                       git log ran.
  over_budget.json     --max-budget-usd 0.0001, opus: subtype error_max_budget_usd, exit 1.
  init_restricted.json the system/init event of a stage-shaped call: --restricted
                       --disable-slash-commands --settings <stage settings> --tools
                       Read,Grep,Glob,Bash --strict-mcp-config --mcp-config {"mcpServers":{}},
                       --permission-mode dontAsk. Pins D1's init shape: tools exactly
                       Bash, Glob, Grep, Read, StructuredOutput; mcp_servers [].

gh/
  pr_view.json              gh pr view 257 --json PR_FIELDS,headRefOid,baseRefName,
                            isCrossRepository,files,changedFiles,commits,reviews (PR #257)
  checks.json               gh pr checks 257 --json name,state,bucket,link (PR #257)
  threads_p1.json           GraphQL reviewThreads page 1 of PR #144: 9 threads, hasNextPage false
  issues.json               the needs-human issue list (NEEDS_HUMAN_ISSUES_ARGS)
  label_agent_stop.json     the REST lookup of the agent-stop label (HTTP 200)
  agent_stop_empty.json     an open-issues query for the agent-stop label with no match ([])
  label_404.err, .rc        a lookup of a missing label: stderr "gh: Not Found (HTTP 404)", exit 1
  run_log_failed.txt        the last 200 lines of gh run view --log-failed for run 40448856000
