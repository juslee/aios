export const meta = {
  name: 'audit-loop',
  description: 'One audit round of an AIOS claude/* branch: typed read-only lenses, then skeptic votes by severity',
  whenToUse: 'Run by the /audit-loop skill, once per round, from the main checkout (rule 02 audit)',
  phases: [
    { title: 'Find', detail: 'read-only lenses, one agent per lens (code-reviewer, doc-auditor)' },
    { title: 'Verify', detail: 'skeptics per finding: three for must-fix, one otherwise' },
  ],
}

// Routing (agent type, reasoning tier) lives in the .claude/agents frontmatter.
// This script passes only agentType, never a per-call override.

const A = args || {}
for (const k of ['worktree', 'branch', 'base', 'head', 'mode']) {
  if (typeof A[k] !== 'string' || !A[k]) throw new Error(`audit-loop: args.${k} is required`)
}
if (!['kernel', 'tools', 'both', 'docs'].includes(A.mode)) {
  throw new Error(`audit-loop: args.mode must be kernel, tools, both or docs (got ${A.mode})`)
}
const W = A.worktree
const POOL = 6
const TRIES = 3

const LOCATION = `You run in the main checkout. Use \`git -C ${W}\` for every git command and absolute paths under \`${W}\` for every file. The change is \`${A.base}..${A.head}\` on \`${A.branch}\`. Gate output (sha \`${A.head}\`): \`${A.gates || '(none given)'}\`.
${A.context ? `Context and owner decisions that limit scope: ${A.context}` : ''}`

function ledgerLine(x) {
  if (typeof x === 'string') return x
  const loc = x.file ? `${x.file}${x.line ? ':' + x.line : ''} ` : ''
  return `${loc}${x.summary || JSON.stringify(x)}`
}

const MEMO = [
  [A.fixed, 'Already fixed in earlier rounds (do not re-report unless the fix is wrong or incomplete, and then say so):'],
  [A.uncertain, 'Unverified last round: re-check each one, and re-report it if it is still present:'],
  [A.refuted, 'Already refuted by skeptics (do not re-report without new evidence):'],
]
  .filter(([ledger]) => Array.isArray(ledger) && ledger.length)
  .map(([ledger, heading]) => `${heading}\n${ledger.map(x => '- ' + ledgerLine(x)).join('\n')}`)
  .join('\n\n')

const COMPLETENESS = `Also check ISSUE COMPLETENESS: for every item in the plan step or issue text named in the context, decide whether the range fixes it, or leaves it out with a justification the context accepts. Each unaddressed, half-addressed or wrongly addressed item is a finding. Flag any acceptance claim not backed by a test or a check.`
const TEST_ADEQUACY = `Also check TEST ADEQUACY: do the new or changed host tests and boot self-tests exercise the fixed behaviour (would each fail on the base)? Report only gaps that matter for the defects the range fixes.`
const LEFTOVERS = `For each item the range fixes, search the WHOLE docs tree (and .claude/CLAUDE.md, README.md, .claude/rules) with \`git -C ${W} grep\` for remaining instances of the stale term or value, and report each remaining one the item should have covered. Also report edits outside the items' scope, and any edit to an item the context says the owner holds.`

const CODE = 'code-reviewer'
const DOCS = 'doc-auditor'

function codeSet(kind) {
  const rulesExtra = kind === 'kernel' ? `${COMPLETENESS}\n${TEST_ADEQUACY}` : COMPLETENESS
  return [
    { key: `rules-${kind}`, agentType: CODE, prompt: `lens: rules\n${rulesExtra}` },
    { key: `bugs-${kind}`, agentType: CODE, prompt: 'lens: bugs' },
  ]
}

function buildLenses() {
  if (A.mode === 'docs') {
    return [
      { key: 'docs-accuracy', agentType: DOCS, prompt: 'lens: accuracy' },
      { key: 'docs-format', agentType: DOCS, prompt: 'lens: format' },
      { key: 'docs-leftovers', agentType: DOCS, prompt: `lens: leftovers\n${LEFTOVERS}` },
    ]
  }
  // Mode both uses the kernel set: its rules text is a superset of the tools text, and bugs is identical.
  return [...codeSet(A.mode === 'tools' ? 'tools' : 'kernel'), { key: 'docs', agentType: DOCS, prompt: '' }]
}

const FINDINGS = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          severity: { type: 'string', enum: ['must-fix', 'should-fix', 'nit'] },
          scope: { type: 'string', enum: ['in-diff', 'pre-existing'] },
          file: { type: 'string', description: 'path relative to the repository root, never absolute' },
          line: { type: 'integer', description: 'line number at the head; 0 when the finding has no line' },
          summary: { type: 'string' },
          evidence: { type: 'string' },
          failure_scenario: { type: 'string' },
          fix: { type: 'string' },
        },
        required: ['severity', 'scope', 'file', 'line', 'summary', 'evidence', 'failure_scenario', 'fix'],
      },
    },
  },
  required: ['findings'],
}

const VERDICT = {
  type: 'object',
  properties: {
    verdict: { type: 'string', enum: ['CONFIRMED', 'REFUTED', 'UNCERTAIN'] },
    scope: { type: 'string', enum: ['in-diff', 'pre-existing'] },
    reachable: { type: 'string', enum: ['yes', 'no', 'unknown'] },
    evidence: { type: 'string' },
    reason: { type: 'string' },
  },
  required: ['verdict', 'scope', 'reachable', 'evidence', 'reason'],
}

const RANK = { 'must-fix': 3, 'should-fix': 2, nit: 1 }

// Lenses sometimes return absolute paths under the worktree; the ledger and dedup use repo-relative ones.
function relFile(f) {
  const file = String(f.file || '')
  const prefix = W.endsWith('/') ? W : W + '/'
  return file.startsWith(prefix) ? file.slice(prefix.length) : file
}

// Same file, same line, same normalised summary: one finding. Two distinct issues on one line stay apart.
function keyOf(f) {
  const s = String(f.summary || '').toLowerCase().replace(/\s+/g, ' ').trim()
  return `${f.file}:${f.line}:${s}`
}

// agent() returns null when the subagent dies (API error, usage limit). A null
// is never an empty result: retry, and let callers treat a final null as "no
// evidence", never as "clean". The first attempt keeps the original prompt and
// opts so completed agents replay from cache on resume.
async function run(prompt, opts) {
  for (let t = 1; t <= TRIES; t++) {
    const o = t === 1 ? opts : { ...opts, label: `${opts.label} retry${t - 1}` }
    const r = await agent(prompt, o)
    if (r) return r
    log(`${opts.label}: attempt ${t} of ${TRIES} returned nothing`)
  }
  return null
}

// At most POOL agents at a time: workers pull thunks from one shared queue.
async function pool(thunks) {
  const out = new Array(thunks.length).fill(null)
  let next = 0
  async function worker() {
    while (next < thunks.length) {
      const i = next++
      try { out[i] = await thunks[i]() } catch (e) { log(`task ${i} threw: ${e && e.message}`); out[i] = null }
    }
  }
  await Promise.all(Array.from({ length: Math.min(POOL, thunks.length) }, worker))
  return out
}

// ---- Find ----
phase('Find')
const lenses = buildLenses()
log(`mode ${A.mode}: ${lenses.length} lens(es), pool of ${POOL}`)

const found = await pool(lenses.map(l => () =>
  run(`${LOCATION}\n\n${l.prompt}${MEMO ? '\n\n' + MEMO : ''}`,
    { label: `find:${l.key}`, phase: 'Find', agentType: l.agentType, schema: FINDINGS })
))

const lens_failures = []
const byKey = new Map()
found.forEach((r, i) => {
  if (!r) { lens_failures.push(lenses[i].key); return }
  for (const raw of r.findings) {
    const f = { ...raw, file: relFile(raw) }
    const k = keyOf(f)
    const prev = byKey.get(k)
    if (!prev) byKey.set(k, { ...f, lenses: [lenses[i].key] })
    else {
      log(`merged duplicate ${k} from ${lenses[i].key} into ${prev.lenses.join('+')}`)
      if (!prev.lenses.includes(lenses[i].key)) prev.lenses.push(lenses[i].key)
      if ((RANK[f.severity] || 0) > (RANK[prev.severity] || 0)) {
        Object.assign(prev, { severity: f.severity, summary: f.summary, evidence: f.evidence, failure_scenario: f.failure_scenario, fix: f.fix })
      }
    }
  }
})
const fresh = [...byKey.values()]
log(`${fresh.length} finding(s) to verify; lens failures: ${lens_failures.length ? lens_failures.join(', ') : 'none'}`)

// ---- Verify ----
phase('Verify')
const skepticsFor = f => (f.severity === 'must-fix' ? 3 : 1)
const tasks = []
fresh.forEach((f, i) => {
  for (let s = 0; s < skepticsFor(f); s++) tasks.push({ i, s })
})
const SKEPTIC_NOTE = `The finding comes from a reviewer lens and may be wrong. Head sha \`${A.head}\`, base \`${A.base}\`.`
const verdicts = await pool(tasks.map(t => () => {
  const f = fresh[t.i]
  return run(`${LOCATION}\n\n${SKEPTIC_NOTE}\n\nFinding:\nfile: ${f.file}\nline: ${f.line}\nseverity: ${f.severity}\nsummary: ${f.summary}\nevidence: ${f.evidence}\nfailure_scenario: ${f.failure_scenario}\nfix: ${f.fix}`,
    { label: `verify#${t.i}.${t.s}`, phase: 'Verify', agentType: 'skeptic', schema: VERDICT })
}))

const in_diff = []
const pre_existing = []
const refuted = []
const uncertain = []
fresh.forEach((f, i) => {
  const asked = skepticsFor(f)
  const need = Math.floor(asked / 2) + 1 // a majority of the skeptics asked
  const vs = tasks.map((t, n) => (t.i === i ? verdicts[n] : null)).filter(Boolean)
  const count = v => vs.filter(x => x.verdict === v).length
  const detail = { ...f, votes: vs.map(x => ({ verdict: x.verdict, scope: x.scope, reachable: x.reachable, reason: x.reason, evidence: x.evidence })), skeptics_answered: vs.length, skeptics_asked: asked }
  if (count('CONFIRMED') >= need) {
    const confirming = vs.filter(x => x.verdict === 'CONFIRMED')
    // A tie (for example 1 of 2 confirming skeptics) goes to pre-existing: an unfixed real defect becomes an issue, not a branch edit.
    const inDiffN = confirming.filter(x => x.scope === 'in-diff').length
    ;(inDiffN * 2 > confirming.length ? in_diff : pre_existing).push(detail)
  } else if (count('REFUTED') >= need) {
    refuted.push(detail)
  } else {
    uncertain.push(detail)
  }
})

const complete = lens_failures.length === 0 && uncertain.length === 0
log(`complete=${complete}: ${in_diff.length} in-diff, ${pre_existing.length} pre-existing, ${refuted.length} refuted, ${uncertain.length} uncertain`)

return { complete, in_diff, lens_failures, pre_existing, refuted, uncertain }
