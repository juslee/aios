---
name: review-pr-comments
description: >
  Wait for Copilot/reviewer comments on a PR, then address each comment:
  fix code, reply, and resolve conversations. Use after creating a PR.
---

# Review PR Comments

After PR creation, wait for automated reviewers, then address all comments in one pass.

## Step 1: Detect PR

```bash
gh pr view --json number,url,headRefName --jq '{number, url, branch: .headRefName}'
```

If this fails, ask the user for the PR number.

## Step 2: Wait for reviewer comments

Poll every 60 seconds for up to 5 minutes. Stop early if any comments appear.

Check all three comment sources and use their **combined count** as the stop condition:

```bash
# PR-level comments + reviews
gh pr view --json comments,reviews --jq '{
  issue_comments: (.comments | length),
  reviews: (.reviews | length)
}'

# Inline review comments (separate API)
gh api repos/{owner}/{repo}/pulls/{number}/comments --jq 'length'
```

**Stop condition**: `issue_comments + reviews + inline_comments > 0`. If after 5 minutes the combined count is still 0, inform the user and stop.

## Step 3: Read all comments

Fetch all comment types:

1. **PR-level comments**: `gh pr view --json comments`
2. **Review comments (inline)**: `gh api repos/{owner}/{repo}/pulls/{number}/comments`
3. **Reviews**: `gh pr view --json reviews`

For each comment, extract: author, body, file path (if inline), line number, comment ID.

## Step 4: Categorize and summarize

Present a summary to the user:

- **Code suggestions**: Comments requesting code changes (fix these)
- **Questions**: Comments asking for clarification (reply with explanation)
- **Nits**: Style/minor issues (fix or explain why not)
- **Approvals/FYI**: No action needed

## Step 5: Fix code issues

For each actionable comment:
1. Read the relevant file and understand the context
2. Implement the fix
3. Stage the change

After all fixes, create a single commit:
```
Address PR review comments

Co-Authored-By: Claude <noreply@anthropic.com>
```

Note: Use the generic `Claude` attribution to match project conventions and avoid staleness as models change.

## Step 6: Reply to each comment

For inline review comments, reply via API:

```bash
gh api repos/{owner}/{repo}/pulls/{number}/comments/{comment_id}/replies \
  -f body="<what was done or explanation>"
```

For PR-level comments:

```bash
gh pr comment {number} --body "<reply>"
```

Guidelines for replies:
- If fixed: "Fixed in <commit-sha-short>." with brief explanation if non-obvious
- If question: Answer directly and concisely
- If won't fix: Explain why with reasoning
- Keep replies short and professional

## Step 7: Resolve conversations

For each addressed comment thread, resolve it via the GraphQL API:

```bash
gh api graphql -f query='
  mutation {
    resolveReviewThread(input: {threadId: "<thread_node_id>"}) {
      thread { isResolved }
    }
  }
'
```

To get thread node IDs, query with pagination and comment identifiers for reliable mapping:

```bash
gh api graphql -f query='
  query($cursor: String) {
    repository(owner: "{owner}", name: "{repo}") {
      pullRequest(number: {number}) {
        reviewThreads(first: 100, after: $cursor) {
          pageInfo {
            hasNextPage
            endCursor
          }
          nodes {
            id
            isResolved
            comments(first: 10) {
              nodes {
                id
                databaseId
                body
              }
            }
          }
        }
      }
    }
  }
'
```

If `pageInfo.hasNextPage` is true, repeat the query with `$cursor` set to `endCursor` until all threads are fetched.

Map each REST `comment_id` to a thread by matching against the `databaseId` field in thread comments. Only resolve threads that were actually addressed (fixed or answered).

## Step 8: Push

```bash
git push
```

Report final summary: how many comments addressed, commits pushed, threads resolved.
