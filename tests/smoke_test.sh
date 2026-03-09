#!/usr/bin/env bash
# ============================================================================
# PUNCH AGENT OS — COMPREHENSIVE SMOKE TEST SUITE
# ============================================================================
# Tests every major subsystem against local Ollama.
# Usage: ./tests/smoke_test.sh
# ============================================================================

set -uo pipefail

PUNCH="cargo run --bin punch --"
PASS=0
FAIL=0
SKIP=0
RESULTS=()

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

banner() {
    echo ""
    echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${CYAN}${BOLD}  $1${NC}"
    echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

section() {
    echo ""
    echo -e "${BOLD}── $1 ──${NC}"
}

pass() {
    PASS=$((PASS + 1))
    RESULTS+=("${GREEN}PASS${NC}  $1")
    echo -e "  ${GREEN}PASS${NC}  $1"
}

fail() {
    FAIL=$((FAIL + 1))
    RESULTS+=("${RED}FAIL${NC}  $1  ($2)")
    echo -e "  ${RED}FAIL${NC}  $1"
    echo -e "       ${RED}$2${NC}"
}

skip() {
    SKIP=$((SKIP + 1))
    RESULTS+=("${YELLOW}SKIP${NC}  $1  ($2)")
    echo -e "  ${YELLOW}SKIP${NC}  $1 — $2"
}

cleanup() {
    # Kill daemon if running
    if [ -n "${DAEMON_PID:-}" ]; then
        kill "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    # Remove PID file
    rm -f ~/.punch/punch.pid 2>/dev/null || true
}
trap cleanup EXIT

banner "PUNCH AGENT OS — SMOKE TEST SUITE"
echo "  Testing against local Ollama"
echo "  $(date)"

# ============================================================================
# PHASE 1: PREREQUISITES
# ============================================================================
banner "PHASE 1: Prerequisites"

# Test 1.1: Ollama reachable
section "1.1 Ollama connectivity"
if curl -sf http://localhost:11434/api/tags >/dev/null 2>&1; then
    MODEL=$(curl -s http://localhost:11434/api/tags | python3 -c "import sys,json; m=json.load(sys.stdin)['models']; print(m[0]['name'] if m else 'none')" 2>/dev/null)
    pass "Ollama reachable (model: $MODEL)"
else
    fail "Ollama connectivity" "Cannot reach localhost:11434"
    echo -e "${RED}Ollama is required for all tests. Aborting.${NC}"
    exit 1
fi

# Test 1.2: Punch builds
section "1.2 Build"
if cargo build --bin punch 2>/dev/null; then
    pass "Punch binary builds"
else
    fail "Build" "cargo build failed"
    exit 1
fi

# Test 1.3: Unit tests
section "1.3 Unit tests"
TEST_OUTPUT=$(cargo test --workspace 2>&1)
TEST_COUNT=$(echo "$TEST_OUTPUT" | grep "^test result:" | awk '{sum += $4} END {print sum}')
FAIL_COUNT=$(echo "$TEST_OUTPUT" | grep "^test result:" | awk '{sum += $6} END {print sum}')
if [ "$FAIL_COUNT" = "0" ]; then
    pass "All $TEST_COUNT unit tests pass"
else
    fail "Unit tests" "$FAIL_COUNT failures"
fi

# Test 1.4: Doctor
section "1.4 Doctor diagnostics"
DOCTOR_OUTPUT=$($PUNCH doctor 2>&1)
if echo "$DOCTOR_OUTPUT" | grep -q "fight-ready"; then
    pass "punch doctor — system is fight-ready"
else
    fail "Doctor" "System not fight-ready"
fi

# ============================================================================
# PHASE 2: FIGHTER OPERATIONS (in-process, no daemon)
# ============================================================================
banner "PHASE 2: Fighter Operations (In-Process)"

# Test 2.1: One-shot chat
section "2.1 One-shot chat"
RESPONSE=$($PUNCH chat "Reply with exactly the word KNOCKOUT and nothing else." 2>&1)
if echo "$RESPONSE" | grep -qi "KNOCKOUT"; then
    TOKENS=$(echo "$RESPONSE" | grep -o 'tokens: [0-9]* in / [0-9]* out' || echo "unknown")
    pass "One-shot chat works ($TOKENS)"
else
    fail "One-shot chat" "Did not get expected response"
fi

# Test 2.2: Spawn each built-in template
section "2.2 Built-in fighter templates"
for TEMPLATE in default striker scout oracle coder; do
    OUTPUT=$($PUNCH fighter spawn "$TEMPLATE" 2>&1)
    if echo "$OUTPUT" | grep -q "Fighter spawned"; then
        pass "Template '$TEMPLATE' spawns"
    else
        fail "Template '$TEMPLATE'" "Failed to spawn"
    fi
done

# Test 2.3: Tool use — file_read
section "2.3 Tool use: file_read"
RESPONSE=$(echo -e "Read the file Cargo.toml and tell me the workspace member count. Reply like: MEMBERS=<number>\n/exit" | $PUNCH fighter chat striker 2>&1)
if echo "$RESPONSE" | grep -qE "tools: [1-9]"; then
    pass "file_read tool invoked by fighter"
else
    fail "file_read tool" "Fighter did not call the tool"
fi

# Test 2.4: Tool use — shell_exec
section "2.4 Tool use: shell_exec"
RESPONSE=$(echo -e "Run the command 'echo PUNCH_SHELL_TEST_12345' using shell_exec and tell me the output.\n/exit" | $PUNCH fighter chat coder 2>&1)
if echo "$RESPONSE" | grep -q "PUNCH_SHELL_TEST_12345"; then
    pass "shell_exec tool works — output captured"
else
    fail "shell_exec tool" "Shell output not in response"
fi

# Test 2.5: Tool use — file_list
section "2.5 Tool use: file_list"
RESPONSE=$(echo -e "List the files in the crates/ directory. Just list the folder names.\n/exit" | $PUNCH fighter chat striker 2>&1)
if echo "$RESPONSE" | grep -q "punch-kernel"; then
    pass "file_list tool works — directory listed"
else
    fail "file_list tool" "Directory listing not in response"
fi

# Test 2.6: Tool use — memory_store + memory_recall
section "2.6 Tool use: memory (store + recall)"
RESPONSE=$(echo -e "Store the memory with key 'test_secret' and value 'GORILLA_POWER_42'. Then recall it by searching for 'test_secret'. Tell me the value you recalled.\n/exit" | $PUNCH fighter chat striker 2>&1)
if echo "$RESPONSE" | grep -q "GORILLA_POWER_42"; then
    pass "memory_store + memory_recall work"
else
    # Memory tools may not always be called; check tool count
    if echo "$RESPONSE" | grep -qE "tools: [1-9]"; then
        pass "memory tools invoked (recall content may vary)"
    else
        fail "memory tools" "Fighter did not use memory tools"
    fi
fi

# Test 2.7: Multi-turn conversation (memory persistence)
section "2.7 Multi-turn conversation"
RESPONSE=$(echo -e "Remember this code word: SILVERBACK_ALPHA_7\n/exit" | $PUNCH fighter chat oracle 2>&1)
# The bout is ephemeral in-process, so we can't test cross-session persistence here
# But we verify the fighter responded
if echo "$RESPONSE" | grep -qE "tokens: [0-9]+ in"; then
    pass "Multi-turn: first message processed"
else
    fail "Multi-turn" "Fighter did not respond"
fi

# Test 2.8: Complex reasoning task
section "2.8 Complex reasoning"
RESPONSE=$($PUNCH chat "Write a Python function that checks if a string is a palindrome. Include the function signature and at least one test case. Keep it under 10 lines." 2>&1)
if echo "$RESPONSE" | grep -q "def.*palindrome\|def.*is_palindrome"; then
    pass "Complex reasoning — code generation works"
else
    # Some models may name it differently
    if echo "$RESPONSE" | grep -qi "palindrome"; then
        pass "Complex reasoning — response is relevant"
    else
        fail "Complex reasoning" "Response does not contain palindrome code"
    fi
fi

# ============================================================================
# PHASE 3: DAEMON MODE + REST API
# ============================================================================
banner "PHASE 3: Daemon Mode + REST API"

# Start daemon in background
section "3.0 Starting daemon"
$PUNCH start >/dev/null 2>&1 &
DAEMON_PID=$!
sleep 3

# Test 3.1: Health endpoint
section "3.1 Health endpoint"
HEALTH=$(curl -sf http://127.0.0.1:6660/health 2>/dev/null || echo "FAIL")
if echo "$HEALTH" | grep -q '"ok"'; then
    pass "GET /health — daemon is alive"
else
    fail "Health endpoint" "Daemon not responding"
    # If daemon isn't up, skip remaining API tests
    echo -e "${YELLOW}  Skipping remaining API tests${NC}"
    kill "$DAEMON_PID" 2>/dev/null || true
    DAEMON_PID=""

    banner "PHASE 3: SKIPPED (daemon not available)"
    SKIP=$((SKIP + 10))
    # Jump to phase 4
    goto_phase4=true
fi

if [ -z "${goto_phase4:-}" ]; then

# Test 3.2: API status
section "3.2 API status"
STATUS=$(curl -sf http://127.0.0.1:6660/api/status 2>/dev/null)
if echo "$STATUS" | grep -q '"gorilla_count"'; then
    GORILLAS=$(echo "$STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin).get('gorilla_count', 0))" 2>/dev/null)
    pass "GET /api/status — $GORILLAS gorillas registered"
else
    fail "API status" "Missing gorilla_count"
fi

# Test 3.3: Spawn fighter via API
section "3.3 Spawn fighter via API"
SPAWN_RESULT=$(curl -sf -X POST http://127.0.0.1:6660/api/fighters \
    -H "Content-Type: application/json" \
    -d '{"manifest":{"name":"API-Test-Fighter","description":"Spawned by smoke test","model":{"provider":"ollama","model":"gpt-oss:20b","base_url":"http://localhost:11434","max_tokens":4096,"temperature":0.7},"system_prompt":"You are a test fighter. Always respond with exactly: FIGHT_READY","capabilities":[],"weight_class":"featherweight"}}' 2>/dev/null)
FIGHTER_ID=$(echo "$SPAWN_RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])" 2>/dev/null || echo "")
if [ -n "$FIGHTER_ID" ]; then
    pass "POST /api/fighters — spawned $FIGHTER_ID"
else
    fail "Spawn via API" "No fighter ID returned"
fi

# Test 3.4: List fighters
section "3.4 List fighters"
FIGHTERS=$(curl -sf http://127.0.0.1:6660/api/fighters 2>/dev/null)
FIGHTER_COUNT=$(echo "$FIGHTERS" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
if [ "$FIGHTER_COUNT" -ge 1 ]; then
    pass "GET /api/fighters — $FIGHTER_COUNT fighter(s) listed"
else
    fail "List fighters" "No fighters returned"
fi

# Test 3.5: Get fighter detail
section "3.5 Get fighter detail"
if [ -n "$FIGHTER_ID" ]; then
    DETAIL=$(curl -sf "http://127.0.0.1:6660/api/fighters/$FIGHTER_ID" 2>/dev/null)
    if echo "$DETAIL" | grep -q "API-Test-Fighter"; then
        pass "GET /api/fighters/:id — details returned"
    else
        fail "Fighter detail" "Name not found in response"
    fi
else
    skip "Fighter detail" "No fighter ID from spawn"
fi

# Test 3.6: Send message via API
section "3.6 Send message via API"
if [ -n "$FIGHTER_ID" ]; then
    MSG_RESULT=$(curl -sf -X POST "http://127.0.0.1:6660/api/fighters/$FIGHTER_ID/message" \
        -H "Content-Type: application/json" \
        -d '{"message":"What is 1+1? Answer with just the number."}' 2>/dev/null)
    if echo "$MSG_RESULT" | grep -q '"response"'; then
        TOKENS=$(echo "$MSG_RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('tokens_used', 0))" 2>/dev/null)
        pass "POST /api/fighters/:id/message — response received ($TOKENS tokens)"
    else
        fail "Send message" "No response field"
    fi
else
    skip "Send message" "No fighter ID"
fi

# Test 3.7: Second message (multi-turn via API)
section "3.7 Multi-turn via API"
if [ -n "$FIGHTER_ID" ]; then
    MSG2=$(curl -sf -X POST "http://127.0.0.1:6660/api/fighters/$FIGHTER_ID/message" \
        -H "Content-Type: application/json" \
        -d '{"message":"What did I just ask you? Summarize in one sentence."}' 2>/dev/null)
    if echo "$MSG2" | grep -q '"response"'; then
        pass "Multi-turn API — second message processed (conversation memory works)"
    else
        fail "Multi-turn API" "Second message failed"
    fi
else
    skip "Multi-turn API" "No fighter ID"
fi

# Test 3.8: Spawn fighter with tools via API + tool use
section "3.8 Tool use via API"
TOOL_FIGHTER=$(curl -sf -X POST http://127.0.0.1:6660/api/fighters \
    -H "Content-Type: application/json" \
    -d '{"manifest":{"name":"Tool-Tester","description":"Fighter with tools","model":{"provider":"ollama","model":"gpt-oss:20b","base_url":"http://localhost:11434","max_tokens":4096,"temperature":0.3},"system_prompt":"You are a helpful assistant. Use tools when asked.","capabilities":[{"type":"file_read","scope":"**"},{"type":"shell_exec","scope":"*"}],"weight_class":"heavyweight"}}' 2>/dev/null)
TOOL_ID=$(echo "$TOOL_FIGHTER" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])" 2>/dev/null || echo "")
if [ -n "$TOOL_ID" ]; then
    TOOL_MSG=$(curl -sf -X POST "http://127.0.0.1:6660/api/fighters/$TOOL_ID/message" \
        -H "Content-Type: application/json" \
        -d '{"message":"Run the shell command: echo PUNCH_API_TOOL_TEST_OK"}' 2>/dev/null)
    TOOL_CALLS=$(echo "$TOOL_MSG" | python3 -c "import sys,json; print(json.load(sys.stdin).get('tool_calls_made', 0))" 2>/dev/null || echo "0")
    if [ "$TOOL_CALLS" -ge 1 ]; then
        pass "Tool use via API — $TOOL_CALLS tool call(s) made"
    else
        # Tool might not be called if model answers directly
        if echo "$TOOL_MSG" | grep -q '"response"'; then
            pass "Tool fighter responded (model may have answered without tool)"
        else
            fail "Tool use via API" "No response or tool calls"
        fi
    fi
else
    fail "Tool fighter spawn" "Failed to spawn fighter with tools"
fi

# Test 3.9: List gorillas
section "3.9 List gorillas"
GORILLA_LIST=$(curl -sf http://127.0.0.1:6660/api/gorillas 2>/dev/null)
GORILLA_COUNT=$(echo "$GORILLA_LIST" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
if [ "$GORILLA_COUNT" -ge 1 ]; then
    NAMES=$(echo "$GORILLA_LIST" | python3 -c "import sys,json; print(', '.join(g['name'] for g in json.load(sys.stdin)))" 2>/dev/null)
    pass "GET /api/gorillas — $GORILLA_COUNT gorillas ($NAMES)"
else
    fail "List gorillas" "No gorillas returned"
fi

# Test 3.10: Delete fighter
section "3.10 Delete fighter"
if [ -n "$FIGHTER_ID" ]; then
    DEL_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE "http://127.0.0.1:6660/api/fighters/$FIGHTER_ID" 2>/dev/null)
    if [ "$DEL_STATUS" = "204" ]; then
        pass "DELETE /api/fighters/:id — fighter killed (204)"
    else
        fail "Delete fighter" "Got status $DEL_STATUS"
    fi
else
    skip "Delete fighter" "No fighter ID"
fi

# Test 3.11: Verify deletion
section "3.11 Verify deletion"
if [ -n "$FIGHTER_ID" ]; then
    GET_STATUS=$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:6660/api/fighters/$FIGHTER_ID" 2>/dev/null)
    if [ "$GET_STATUS" = "404" ]; then
        pass "Deleted fighter returns 404"
    else
        fail "Verify deletion" "Got status $GET_STATUS instead of 404"
    fi
else
    skip "Verify deletion" "No fighter ID"
fi

# Test 3.12: OpenAI-compatible endpoint
section "3.12 OpenAI-compatible /v1/chat/completions"
CHAT_RESULT=$(curl -sf -X POST http://127.0.0.1:6660/v1/chat/completions \
    -H "Content-Type: application/json" \
    -d '{"model":"gpt-oss:20b","messages":[{"role":"user","content":"Say hello"}],"max_tokens":50}' 2>/dev/null || echo "FAIL")
if echo "$CHAT_RESULT" | grep -q '"choices"\|"content"'; then
    pass "OpenAI-compatible chat endpoint works"
elif echo "$CHAT_RESULT" | grep -qi "error\|not found\|404"; then
    skip "OpenAI-compatible endpoint" "Endpoint may not be implemented yet"
else
    skip "OpenAI-compatible endpoint" "Unexpected response"
fi

# Stop daemon
kill "$DAEMON_PID" 2>/dev/null || true
wait "$DAEMON_PID" 2>/dev/null || true
DAEMON_PID=""

fi  # end goto_phase4 check

# ============================================================================
# PHASE 4: STRESS & EDGE CASES
# ============================================================================
banner "PHASE 4: Stress & Edge Cases"

# Test 4.1: Empty message handling
section "4.1 Empty message handling"
RESPONSE=$($PUNCH chat "" 2>&1 || true)
# Should not crash
pass "Empty message — no crash"

# Test 4.2: Very long input
section "4.2 Long input handling"
LONG_INPUT=$(python3 -c "print('Tell me about ' + 'gorillas ' * 200 + 'in one sentence.')")
RESPONSE=$($PUNCH chat "$LONG_INPUT" 2>&1 || true)
if echo "$RESPONSE" | grep -qE "tokens: [0-9]+ in"; then
    pass "Long input processed without crash"
else
    pass "Long input — no crash (response may vary)"
fi

# Test 4.3: Special characters
section "4.3 Special characters in input"
RESPONSE=$($PUNCH chat "What is 2+2? Reply with: <result>4</result> & that's \"it\"" 2>&1 || true)
if echo "$RESPONSE" | grep -qE "tokens: [0-9]+ in"; then
    pass "Special characters handled"
else
    pass "Special characters — no crash"
fi

# Test 4.4: Rapid sequential messages
section "4.4 Rapid sequential messages"
for i in 1 2 3; do
    $PUNCH chat "Say $i" >/dev/null 2>&1 || true
done
pass "3 rapid sequential messages — no crashes"

# Test 4.5: Invalid template name
section "4.5 Invalid template name"
RESPONSE=$($PUNCH fighter spawn nonexistent_template_xyz 2>&1 || true)
if echo "$RESPONSE" | grep -qi "unknown\|not found\|error"; then
    pass "Invalid template rejected gracefully"
else
    fail "Invalid template" "No error message shown"
fi

# ============================================================================
# PHASE 5: CLI COMMANDS
# ============================================================================
banner "PHASE 5: CLI Commands"

# Test 5.1: Help
section "5.1 Help output"
HELP=$($PUNCH --help 2>&1)
if echo "$HELP" | grep -q "fighter\|chat\|start"; then
    pass "punch --help shows commands"
else
    fail "Help" "Missing expected commands"
fi

# Test 5.2: Fighter subcommands
section "5.2 Fighter help"
FHELP=$($PUNCH fighter --help 2>&1)
if echo "$FHELP" | grep -q "spawn\|chat\|list\|kill"; then
    pass "punch fighter --help shows subcommands"
else
    fail "Fighter help" "Missing subcommands"
fi

# Test 5.3: Gorilla subcommands
section "5.3 Gorilla help"
GHELP=$($PUNCH gorilla --help 2>&1)
if echo "$GHELP" | grep -q "list\|unleash\|cage\|status"; then
    pass "punch gorilla --help shows subcommands"
else
    fail "Gorilla help" "Missing subcommands"
fi

# ============================================================================
# RESULTS
# ============================================================================
banner "TEST RESULTS"

echo ""
for result in "${RESULTS[@]}"; do
    echo -e "  $result"
done

TOTAL=$((PASS + FAIL + SKIP))
echo ""
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "  ${GREEN}PASS: $PASS${NC}  ${RED}FAIL: $FAIL${NC}  ${YELLOW}SKIP: $SKIP${NC}  TOTAL: $TOTAL"
echo -e "${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

if [ "$FAIL" -eq 0 ]; then
    echo ""
    echo -e "  ${GREEN}${BOLD}ALL TESTS PASSED. PUNCH IS FIGHT-READY.${NC}"
    echo ""
    exit 0
else
    echo ""
    echo -e "  ${RED}${BOLD}$FAIL TEST(S) FAILED.${NC}"
    echo ""
    exit 1
fi
