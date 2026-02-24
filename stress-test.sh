#!/usr/bin/env bash
set -euo pipefail

# ─── Stress test for Manifest Perps DEX on MagicBlock ER ─────────────────────
# Two independent users trade against bot liquidity at various leverages.
# Uses aggressive Limit orders that cross the spread (IOC unreliable on ER).

CLI="./target/debug/manifest-cli"
MARKET="CCkhp6HH9GSp81dj31xGcYgVsmBErsWiEvdbZ1ed6ouU"
USER1_KP="test-user1.json"
USER2_KP="test-user2.json"

# Oracle ~$80.07, bot bids $83.83-$84.24, bot asks $84.58-$84.92
# Aggressive crossing prices:
ORACLE_USD=80.07
BUY_M=8550;  BUY_E="-5"   # $85.50 - above all asks → fills at resting ask
SELL_M=8300; SELL_E="-5"   # $83.00 - below all bids → fills at resting bid
QUOTE_DEC=6
BASE_DEC=9
DELAY=3  # seconds between trades (let ER settle)

PASS=0
FAIL=0
TOTAL=0
REPORT=""

# ─── helpers ─────────────────────────────────────────────────────────────────

log()  { printf "\n\033[1;36m>>> %s\033[0m\n" "$*"; }
ok()   { printf "\033[1;32m  [PASS]\033[0m %s\n" "$*"; PASS=$((PASS+1)); TOTAL=$((TOTAL+1)); }
fail() { printf "\033[1;31m  [FAIL]\033[0m %s\n" "$*"; FAIL=$((FAIL+1)); TOTAL=$((TOTAL+1)); }

# compute base atoms for target leverage (uses oracle price for on-chain margin check)
base_atoms() {
    local margin_atoms=$1 leverage=$2
    python3 -c "print(int($margin_atoms / 1e${QUOTE_DEC} * $leverage / $ORACLE_USD * 1e${BASE_DEC}))"
}

# Place a crossing limit order against bot liquidity
# action: open_long | close_long | open_short | close_short
trade() {
    local kp=$1 base=$2 action=$3
    local m e bid_flag=""
    case "$action" in
        open_long|close_short)
            m=$BUY_M; e=$BUY_E; bid_flag="--is-bid"
            echo "  [$action] BID ${base} atoms @ ${m}e${e} (\$85.50 crosses asks)"
            ;;
        open_short|close_long)
            m=$SELL_M; e=$SELL_E
            echo "  [$action] ASK ${base} atoms @ ${m}e${e} (\$83.00 crosses bids)"
            ;;
    esac
    $CLI -u er -k "$kp" place-order \
        --market "$MARKET" \
        --base-atoms "$base" \
        --price-mantissa "$m" \
        --price-exponent="$e" \
        $bid_flag \
        --order-type Limit \
        --last-valid-slot 0 2>&1
}

# Get position direction
get_dir() {
    $CLI -u er -k "$1" position --market "$MARKET" 2>&1 | grep "Direction" | awk '{print $NF}'
}

# Get margin atoms
get_margin() {
    $CLI -u er -k "$1" position --market "$MARKET" 2>&1 | grep "Margin (deposit)" | sed 's/.*(\([0-9]*\) atoms).*/\1/'
}

# Get full position output
get_pos() {
    $CLI -u er -k "$1" position --market "$MARKET" 2>&1
}

# ─── run a single test scenario ──────────────────────────────────────────────
# run_test <test_num> <user_kp> <other_kp> <direction> <leverage> <user_label> <other_label>
run_test() {
    local num=$1 kp=$2 other_kp=$3 dir=$4 lev=$5 label=$6 other_label=$7

    local margin
    margin=$(get_margin "$kp")
    local base
    base=$(base_atoms "$margin" "$lev")
    echo "  Size: $base atoms (${lev}x on $margin margin)"

    # Open position
    if [[ "$dir" == "LONG" ]]; then
        trade "$kp" "$base" open_long
    else
        trade "$kp" "$base" open_short
    fi
    sleep $DELAY

    local my_dir other_dir
    my_dir=$(get_dir "$kp")
    other_dir=$(get_dir "$other_kp")
    echo "  After open: $label=$my_dir, $other_label=$other_dir"

    if [[ "$my_dir" == "$dir" ]]; then
        ok "T${num} open: $label=$dir"
    else
        fail "T${num} open: expected $label=$dir, got $my_dir"
    fi

    # Check independence: other user should still be FLAT
    if [[ "$other_dir" == "FLAT" ]]; then
        ok "T${num} independence: $other_label still FLAT"
    else
        fail "T${num} independence: $other_label should be FLAT, got $other_dir"
    fi

    local dir_lower
    dir_lower=$(echo "$dir" | tr '[:upper:]' '[:lower:]')
    REPORT="${REPORT}T${num} open (${lev}x ${dir_lower}): $label=$my_dir, $other_label=$other_dir\n"

    # Close position
    echo ""
    echo "  Closing..."
    if [[ "$dir" == "LONG" ]]; then
        trade "$kp" "$base" close_long
    else
        trade "$kp" "$base" close_short
    fi
    sleep $DELAY

    my_dir=$(get_dir "$kp")
    echo "  After close: $label=$my_dir"

    if [[ "$my_dir" == "FLAT" ]]; then
        ok "T${num} close: $label FLAT"
    else
        fail "T${num} close: expected FLAT, got $my_dir"
    fi

    REPORT="${REPORT}T${num} close: $label=$my_dir\n"
}

# ═══════════════════════════════════════════════════════════════════════════════

USER1_PUB=$(solana-keygen pubkey "$USER1_KP")
USER2_PUB=$(solana-keygen pubkey "$USER2_KP")

echo "═══════════════════════════════════════════════════════"
echo "  MANIFEST PERPS STRESS TEST (vs Bot Liquidity)"
echo "═══════════════════════════════════════════════════════"
echo "  Market     : $MARKET"
echo "  Oracle     : ~\$${ORACLE_USD}"
echo "  User 1     : $USER1_PUB"
echo "  User 2     : $USER2_PUB"
echo "  Buy price  : ${BUY_M}e${BUY_E} (\$85.50)"
echo "  Sell price : ${SELL_M}e${SELL_E} (\$83.00)"
echo "═══════════════════════════════════════════════════════"

# ─── Initial state ───────────────────────────────────────────────────────────

log "INITIAL STATE"
echo "--- User 1 ---"
POS1=$(get_pos "$USER1_KP"); echo "$POS1"
echo ""
echo "--- User 2 ---"
POS2=$(get_pos "$USER2_KP"); echo "$POS2"

INIT_M1=$(get_margin "$USER1_KP")
INIT_M2=$(get_margin "$USER2_KP")
REPORT="Initial margin: User1=${INIT_M1} atoms (\$$(python3 -c "print(f'{$INIT_M1/1e6:.4f}')")), User2=${INIT_M2} atoms (\$$(python3 -c "print(f'{$INIT_M2/1e6:.4f}')"))\n\n"

# ═══════════════════════════════════════════════════════════════════════════════
# TEST 1: User1 LONG at 2x (trades against bot asks)
# ═══════════════════════════════════════════════════════════════════════════════

log "TEST 1: User1 LONG 2x"
run_test 1 "$USER1_KP" "$USER2_KP" LONG 2 "User1" "User2"

# ═══════════════════════════════════════════════════════════════════════════════
# TEST 2: User2 LONG at 5x (trades against bot asks)
# ═══════════════════════════════════════════════════════════════════════════════

log "TEST 2: User2 LONG 5x"
run_test 2 "$USER2_KP" "$USER1_KP" LONG 5 "User2" "User1"

# ═══════════════════════════════════════════════════════════════════════════════
# TEST 3: User1 SHORT at 3x (trades against bot bids)
# ═══════════════════════════════════════════════════════════════════════════════

log "TEST 3: User1 SHORT 3x"
run_test 3 "$USER1_KP" "$USER2_KP" SHORT 3 "User1" "User2"

# ═══════════════════════════════════════════════════════════════════════════════
# TEST 4: User2 SHORT at 7x (trades against bot bids)
# ═══════════════════════════════════════════════════════════════════════════════

log "TEST 4: User2 SHORT 7x"
run_test 4 "$USER2_KP" "$USER1_KP" SHORT 7 "User2" "User1"

# ═══════════════════════════════════════════════════════════════════════════════
# TEST 5: Rapid cycle — User1 LONG 4x (open + immediate close)
# ═══════════════════════════════════════════════════════════════════════════════

log "TEST 5: User1 rapid LONG 4x (open+close)"
run_test 5 "$USER1_KP" "$USER2_KP" LONG 4 "User1" "User2"

# ═══════════════════════════════════════════════════════════════════════════════
# TEST 6: Rapid cycle — User2 SHORT 6x (open + immediate close)
# ═══════════════════════════════════════════════════════════════════════════════

log "TEST 6: User2 rapid SHORT 6x (open+close)"
run_test 6 "$USER2_KP" "$USER1_KP" SHORT 6 "User2" "User1"

# ═══════════════════════════════════════════════════════════════════════════════
# FINAL STATE
# ═══════════════════════════════════════════════════════════════════════════════

log "FINAL STATE"
echo "--- User 1 ---"
POS1=$(get_pos "$USER1_KP"); echo "$POS1"
echo ""
echo "--- User 2 ---"
POS2=$(get_pos "$USER2_KP"); echo "$POS2"

FINAL_M1=$(get_margin "$USER1_KP")
FINAL_M2=$(get_margin "$USER2_KP")
FINAL_DIR1=$(get_dir "$USER1_KP")
FINAL_DIR2=$(get_dir "$USER2_KP")

if [[ "$FINAL_DIR1" == "FLAT" && "$FINAL_DIR2" == "FLAT" ]]; then
    ok "Final: Both users FLAT"
else
    fail "Final: expected both FLAT, got User1=$FINAL_DIR1, User2=$FINAL_DIR2"
fi

# ═══════════════════════════════════════════════════════════════════════════════
# REPORT
# ═══════════════════════════════════════════════════════════════════════════════

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  STRESS TEST REPORT"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "  Market        : $MARKET"
echo "  Oracle Price  : ~\$${ORACLE_USD}"
echo "  User 1        : $USER1_PUB"
echo "  User 2        : $USER2_PUB"
echo ""
echo "  Tests run     : $TOTAL"
echo "  Passed        : $PASS"
echo "  Failed        : $FAIL"
echo ""
echo "  -- Margin Summary --"
DELTA1=$((FINAL_M1 - INIT_M1))
DELTA2=$((FINAL_M2 - INIT_M2))
echo "  User 1: ${INIT_M1} -> ${FINAL_M1} atoms (delta: ${DELTA1}, \$$(python3 -c "print(f'{$DELTA1/1e6:.4f}')"))"
echo "  User 2: ${INIT_M2} -> ${FINAL_M2} atoms (delta: ${DELTA2}, \$$(python3 -c "print(f'{$DELTA2/1e6:.4f}')"))"
echo ""
echo "  -- Test Details --"
printf "  %b" "$REPORT"
echo ""
echo "  -- Final Positions --"
echo "  User 1: $FINAL_DIR1 (margin: ${FINAL_M1} atoms, \$$(python3 -c "print(f'{$FINAL_M1/1e6:.4f}')"))"
echo "  User 2: $FINAL_DIR2 (margin: ${FINAL_M2} atoms, \$$(python3 -c "print(f'{$FINAL_M2/1e6:.4f}')"))"
echo ""
echo "═══════════════════════════════════════════════════════════════"

if [[ $FAIL -eq 0 ]]; then
    printf "\033[1;32m  ALL TESTS PASSED\033[0m\n"
else
    printf "\033[1;31m  %d TEST(S) FAILED\033[0m\n" "$FAIL"
    exit 1
fi
