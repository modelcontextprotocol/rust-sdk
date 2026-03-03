#!/usr/bin/env bash
# =============================================================================
# triage-new-issues.sh — Ongoing Issue Triage for modelcontextprotocol/rust-sdk
#
# Finds open issues that are missing required triage labels (type + priority)
# and uses an LLM to classify them automatically.
#
# Modes:
#   Single issue:  ./scripts/triage-new-issues.sh --issue 700
#   All untriaged: ./scripts/triage-new-issues.sh
#   Apply labels:  ./scripts/triage-new-issues.sh --apply
#   Both:          ./scripts/triage-new-issues.sh --issue 700 --apply
#
# Environment:
#   OPENAI_API_KEY   — Required. API key for the LLM (OpenAI-compatible endpoint)
#   OPENAI_BASE_URL  — Optional. Override the API base URL (default: https://api.openai.com/v1)
#   TRIAGE_MODEL     — Optional. Model to use (default: gpt-4o-mini)
#   GITHUB_TOKEN     — Optional. Used by `gh` CLI for GitHub API access
#
# =============================================================================
set -euo pipefail

REPO="modelcontextprotocol/rust-sdk"
DRY_RUN=true
SINGLE_ISSUE=""
MODEL="${TRIAGE_MODEL:-gpt-4o-mini}"
BASE_URL="${OPENAI_BASE_URL:-https://api.openai.com/v1}"
TRIAGED=0
SKIPPED=0
FAILED=0

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
  case "$1" in
    --apply)  DRY_RUN=false; shift ;;
    --issue)  SINGLE_ISSUE="$2"; shift 2 ;;
    --model)  MODEL="$2"; shift 2 ;;
    --help|-h)
      echo "Usage: $0 [--apply] [--issue NUMBER] [--model MODEL]"
      echo ""
      echo "  --apply        Apply labels to GitHub (default: dry-run)"
      echo "  --issue NUM    Triage a single issue by number"
      echo "  --model MODEL  LLM model to use (default: gpt-4o-mini)"
      echo ""
      echo "Environment:"
      echo "  OPENAI_API_KEY   Required. API key for the LLM"
      echo "  OPENAI_BASE_URL  Optional. API base URL"
      echo "  TRIAGE_MODEL     Optional. Model override"
      exit 0
      ;;
    *) echo "Unknown argument: $1"; exit 1 ;;
  esac
done

# ---------------------------------------------------------------------------
# Preflight checks
# ---------------------------------------------------------------------------
if ! command -v gh &>/dev/null; then
  echo "Error: 'gh' CLI is required. Install from https://cli.github.com/"
  exit 1
fi

if ! command -v jq &>/dev/null; then
  echo "Error: 'jq' is required. Install with: brew install jq"
  exit 1
fi

if [[ -z "${OPENAI_API_KEY:-}" ]]; then
  echo "Error: OPENAI_API_KEY environment variable is required."
  echo "Set it to an OpenAI API key, or set OPENAI_BASE_URL for a compatible endpoint."
  exit 1
fi

echo "============================================="
echo "  rust-sdk Ongoing Issue Triage"
echo "  Repo:  $REPO"
echo "  Model: $MODEL"
if $DRY_RUN; then
  echo "  Mode:  DRY-RUN (pass --apply to execute)"
else
  echo "  Mode:  APPLYING CHANGES"
fi
echo "============================================="
echo ""

# ---------------------------------------------------------------------------
# Label definitions — used to build the LLM prompt
# ---------------------------------------------------------------------------
TYPE_LABELS='["bug", "enhancement", "question"]'
PRIORITY_LABELS='["P0", "P1", "P2", "P3"]'
WORKFLOW_LABELS='["needs confirmation", "needs repro", "ready for work"]'
COMPONENT_LABELS='["T-core", "T-transport", "T-macros", "T-handler", "T-model", "T-security", "T-documentation", "T-examples", "T-service", "T-test", "T-CI", "T-config", "T-dependencies"]'

# ---------------------------------------------------------------------------
# Build the system prompt for the LLM
# ---------------------------------------------------------------------------
read -r -d '' SYSTEM_PROMPT << 'SYSTEM_EOF' || true
You are an issue triage bot for the modelcontextprotocol/rust-sdk repository — a Rust implementation of the Model Context Protocol (MCP).

Your job is to classify GitHub issues by assigning labels. You MUST return valid JSON with exactly these fields:

{
  "type": "<one of: bug, enhancement, question>",
  "priority": "<one of: P0, P1, P2, P3>",
  "components": ["<zero or more from the component list>"],
  "workflow": "<one of: needs confirmation, needs repro, ready for work, or null>",
  "reasoning": "<one sentence explaining your classification>"
}

## Label Definitions

### Type
- bug: Something is not working (errors, crashes, incorrect behavior)
- enhancement: New feature or improvement request
- question: User asking for help or clarification

### Priority
- P0: Critical — blocking, security vulnerability, data loss, or crash affecting all users
- P1: High — MCP spec violation, conformance blocker, or significant functionality broken
- P2: Medium — important but non-blocking improvement, interop issue, or DX gap
- P3: Low — nice-to-have, exploratory, long-term, or questions

### Components (prefix: T-)
- T-core: Core library (rmcp crate internals, JSON-RPC, error handling)
- T-transport: Transport layer (stdio, SSE, streamable HTTP)
- T-macros: Proc macros (#[tool], #[prompt], etc.)
- T-handler: Handler/service implementation
- T-model: Model/data structures and JSON-RPC types
- T-security: OAuth, auth, security features
- T-documentation: Documentation and guides
- T-examples: Example code
- T-service: Service layer
- T-test: Testing
- T-CI: CI/CD workflows
- T-config: Configuration
- T-dependencies: Dependency updates

### Workflow
- "needs confirmation": Bug report that needs verification from a maintainer
- "needs repro": Bug report without a minimal reproduction case
- "ready for work": Issue is well-scoped and ready for a contributor to pick up
- null: None of the above apply

## Rules
1. Every issue MUST get exactly one type and one priority.
2. Assign 0-2 component labels (only if clearly relevant).
3. Assign a workflow label only when appropriate; default to null.
4. When in doubt between two priorities, pick the higher one.
5. Security issues are always P0.
6. MCP spec violations are P1.
7. Questions from users are typically P3.
8. Return ONLY the JSON object, no markdown fences, no extra text.
SYSTEM_EOF

# ---------------------------------------------------------------------------
# classify_issue — call the LLM to classify a single issue
# ---------------------------------------------------------------------------
classify_issue() {
  local title="$1"
  local body="$2"
  local number="$3"
  local existing_labels="$4"

  # Truncate body to ~3000 chars to stay within token limits
  local truncated_body
  truncated_body="$(echo "$body" | head -c 3000)"

  local user_prompt="Classify this GitHub issue.

Issue #${number}: ${title}

Existing labels: ${existing_labels}

Body:
${truncated_body}"

  # Build the JSON payload
  local payload
  payload=$(jq -n \
    --arg model "$MODEL" \
    --arg system "$SYSTEM_PROMPT" \
    --arg user "$user_prompt" \
    '{
      model: $model,
      temperature: 0.1,
      messages: [
        { role: "system", content: $system },
        { role: "user", content: $user }
      ]
    }')

  # Call the LLM
  local response
  response=$(curl -s -w "\n%{http_code}" \
    "${BASE_URL}/chat/completions" \
    -H "Authorization: Bearer ${OPENAI_API_KEY}" \
    -H "Content-Type: application/json" \
    -d "$payload" 2>/dev/null)

  local http_code
  http_code=$(echo "$response" | tail -1)
  local body_response
  body_response=$(echo "$response" | sed '$d')

  if [[ "$http_code" != "200" ]]; then
    echo "ERROR: LLM API returned HTTP $http_code" >&2
    echo "$body_response" | jq -r '.error.message // .' >&2 2>/dev/null || echo "$body_response" >&2
    return 1
  fi

  # Extract the content from the response
  local content
  content=$(echo "$body_response" | jq -r '.choices[0].message.content' 2>/dev/null)

  if [[ -z "$content" || "$content" == "null" ]]; then
    echo "ERROR: Empty response from LLM" >&2
    return 1
  fi

  # Strip markdown fences if present
  content=$(echo "$content" | sed 's/^```json//; s/^```//; s/```$//' | tr -d '\n')

  # Validate it's valid JSON with required fields
  if ! echo "$content" | jq -e '.type and .priority' &>/dev/null; then
    echo "ERROR: LLM returned invalid classification: $content" >&2
    return 1
  fi

  echo "$content"
}

# ---------------------------------------------------------------------------
# apply_labels — apply the classification labels to an issue
# ---------------------------------------------------------------------------
apply_labels() {
  local issue_num="$1"
  local classification="$2"

  local type_label priority_label workflow_label reasoning
  type_label=$(echo "$classification" | jq -r '.type')
  priority_label=$(echo "$classification" | jq -r '.priority')
  workflow_label=$(echo "$classification" | jq -r '.workflow // empty')
  reasoning=$(echo "$classification" | jq -r '.reasoning // "No reasoning provided"')

  # Collect component labels
  local components
  components=$(echo "$classification" | jq -r '.components[]? // empty' 2>/dev/null)

  # Build label list
  local labels=("$type_label" "$priority_label")
  if [[ -n "$workflow_label" && "$workflow_label" != "null" ]]; then
    labels+=("$workflow_label")
  fi
  while IFS= read -r comp; do
    [[ -n "$comp" ]] && labels+=("$comp")
  done <<< "$components"

  # Build gh command
  local cmd_args=(gh issue edit "$issue_num" --repo "$REPO")
  for label in "${labels[@]}"; do
    cmd_args+=(--add-label "$label")
  done

  echo "  Labels:    ${labels[*]}"
  echo "  Reasoning: $reasoning"

  if $DRY_RUN; then
    echo "  [DRY-RUN] ${cmd_args[*]}"
  else
    echo "  [APPLY]   Labeling #$issue_num..."
    if "${cmd_args[@]}" 2>/dev/null; then
      echo "  ✅ Done"
    else
      echo "  ❌ Failed to apply labels"
      return 1
    fi
  fi
}

# ---------------------------------------------------------------------------
# has_triage_labels — check if an issue already has type + priority labels
# ---------------------------------------------------------------------------
has_triage_labels() {
  local labels_json="$1"

  local has_type has_priority
  has_type=$(echo "$labels_json" | jq '[.[] | select(. == "bug" or . == "enhancement" or . == "question")] | length')
  has_priority=$(echo "$labels_json" | jq '[.[] | select(test("^P[0-3]$"))] | length')

  [[ "$has_type" -gt 0 && "$has_priority" -gt 0 ]]
}

# ---------------------------------------------------------------------------
# triage_issue — fetch, classify, and label a single issue
# ---------------------------------------------------------------------------
triage_issue() {
  local issue_num="$1"

  # Fetch issue details
  local issue_json
  issue_json=$(gh issue view "$issue_num" --repo "$REPO" --json title,body,labels 2>/dev/null)

  if [[ -z "$issue_json" ]]; then
    echo "  ❌ Could not fetch issue #$issue_num"
    FAILED=$((FAILED + 1))
    return 1
  fi

  local title body labels_json labels_str
  title=$(echo "$issue_json" | jq -r '.title')
  body=$(echo "$issue_json" | jq -r '.body // ""')
  labels_json=$(echo "$issue_json" | jq '[.labels[].name]')
  labels_str=$(echo "$labels_json" | jq -r 'join(", ")')

  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo "  Issue #$issue_num: $title"
  echo "  Current labels: ${labels_str:-none}"

  # Check if already triaged
  if has_triage_labels "$labels_json"; then
    echo "  ⏭️  Already triaged (has type + priority). Skipping."
    SKIPPED=$((SKIPPED + 1))
    return 0
  fi

  # Classify with LLM
  echo "  🤖 Classifying with $MODEL..."
  local classification
  if ! classification=$(classify_issue "$title" "$body" "$issue_num" "$labels_str"); then
    echo "  ❌ Classification failed"
    FAILED=$((FAILED + 1))
    return 1
  fi

  # Apply labels
  if apply_labels "$issue_num" "$classification"; then
    TRIAGED=$((TRIAGED + 1))
  else
    FAILED=$((FAILED + 1))
  fi
}

# ---------------------------------------------------------------------------
# Main: single issue or scan all untriaged
# ---------------------------------------------------------------------------
if [[ -n "$SINGLE_ISSUE" ]]; then
  echo "--- Triaging single issue #$SINGLE_ISSUE ---"
  echo ""
  triage_issue "$SINGLE_ISSUE"
else
  echo "--- Scanning for untriaged open issues ---"
  echo ""

  # Fetch all open issues (paginated, up to 500)
  issue_numbers=$(gh issue list --repo "$REPO" --state open --limit 500 --json number,labels \
    | jq -r '.[] | select(
        ([.labels[].name | select(. == "bug" or . == "enhancement" or . == "question")] | length) == 0
        or
        ([.labels[].name | select(startswith("P"))] | length) == 0
      ) | .number')

  if [[ -z "$issue_numbers" ]]; then
    echo "✅ All open issues are already triaged! Nothing to do."
    exit 0
  fi

  count=$(echo "$issue_numbers" | wc -l | tr -d ' ')
  echo "Found $count untriaged issue(s)."
  echo ""

  while IFS= read -r num; do
    [[ -z "$num" ]] && continue
    triage_issue "$num"
    echo ""
    # Rate-limit: small delay between LLM calls
    sleep 1
  done <<< "$issue_numbers"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "============================================="
echo "  Triage Summary"
echo ""
echo "  Triaged:  $TRIAGED"
echo "  Skipped:  $SKIPPED (already triaged)"
echo "  Failed:   $FAILED"
echo ""
if $DRY_RUN; then
  echo "  This was a DRY RUN. To apply changes:"
  echo "    $0 --apply"
fi
echo "============================================="

# Exit with error if any failures
[[ "$FAILED" -eq 0 ]] || exit 1
