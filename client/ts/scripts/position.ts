import { Connection, PublicKey } from "@solana/web3.js";

// ── Constants ───────────────────────────────────────────────────────────────
const FIXED_HEADER_SIZE = 256;
const NIL = 0xffffffff;
const RB_HEADER_SIZE = 16;
const CLAIMED_SEAT_SIZE = 64;
const FUNDING_SCALE = 1_000_000_000n;

// ── MarketFixed deserialization ─────────────────────────────────────────────
interface MarketFixed {
  baseMintDecimals: number;
  quoteMintDecimals: number;
  claimedSeatsRootIndex: number;
  initialMarginBps: bigint;
  maintenanceMarginBps: bigint;
  oraclePriceMantissa: bigint;
  oraclePriceExpo: number;
  cumulativeFunding: bigint;
  insuranceFundBalance: bigint;
  takerFeeBps: bigint;
  liquidationBufferBps: bigint;
}

function deserializeMarketFixed(data: Buffer): MarketFixed {
  return {
    baseMintDecimals: data.readUInt8(10),
    quoteMintDecimals: data.readUInt8(11),
    claimedSeatsRootIndex: data.readUInt32LE(76),
    initialMarginBps: data.readBigUInt64LE(96),
    maintenanceMarginBps: data.readBigUInt64LE(104),
    oraclePriceMantissa: data.readBigUInt64LE(160),
    oraclePriceExpo: data.readInt32LE(168),
    cumulativeFunding: data.readBigInt64LE(184),
    insuranceFundBalance: data.readBigUInt64LE(192),
    takerFeeBps: data.readBigUInt64LE(200),
    liquidationBufferBps: data.readBigUInt64LE(208),
  };
}

// ── ClaimedSeat (perps) ─────────────────────────────────────────────────────
interface ClaimedSeat {
  trader: PublicKey;
  lastCumulativeFunding: bigint; // i64, stored in base_withdrawable_balance
  margin: bigint; // u64, quote_withdrawable_balance
  positionSize: bigint; // i64, stored in quote_volume
  costBasis: bigint; // u64, stored in _padding
}

function deserializeClaimedSeat(
  data: Buffer,
  nodeOffset: number,
): ClaimedSeat {
  const payloadOffset = nodeOffset + RB_HEADER_SIZE;
  return {
    trader: new PublicKey(data.subarray(payloadOffset, payloadOffset + 32)),
    lastCumulativeFunding: data.readBigInt64LE(payloadOffset + 32),
    margin: data.readBigUInt64LE(payloadOffset + 40),
    positionSize: data.readBigInt64LE(payloadOffset + 48),
    costBasis: data.readBigUInt64LE(payloadOffset + 56),
  };
}

// ── RB-tree traversal ───────────────────────────────────────────────────────
function findTraderSeat(
  dynamic: Buffer,
  rootIndex: number,
  trader: PublicKey,
): ClaimedSeat | null {
  if (rootIndex === NIL) return null;

  const traderBytes = trader.toBuffer();
  const seats: ClaimedSeat[] = [];

  // In-order traversal: find the leftmost node first
  let currentIndex = rootIndex;
  while (dynamic.readUInt32LE(currentIndex) !== NIL) {
    currentIndex = dynamic.readUInt32LE(currentIndex); // left child
  }

  // Walk via successor
  while (currentIndex !== NIL) {
    const seat = deserializeClaimedSeat(dynamic, currentIndex);
    if (seat.trader.toBuffer().equals(traderBytes)) {
      return seat;
    }
    seats.push(seat);
    currentIndex = getSuccessor(dynamic, currentIndex);
  }

  return null;
}

function getSuccessor(data: Buffer, index: number): number {
  if (index === NIL) return NIL;

  const right = data.readUInt32LE(index + 4);

  // If right child exists, go right then all the way left
  if (right !== NIL) {
    let cur = right;
    while (data.readUInt32LE(cur) !== NIL) {
      cur = data.readUInt32LE(cur); // left child
    }
    return cur;
  }

  // Otherwise go up while we are the right child
  let cur = index;
  let parent = data.readUInt32LE(cur + 8);
  while (parent !== NIL && data.readUInt32LE(parent + 4) === cur) {
    cur = parent;
    parent = data.readUInt32LE(cur + 8);
  }
  return parent;
}

// ── Position metrics ────────────────────────────────────────────────────────
interface PositionMetrics {
  direction: "LONG" | "SHORT" | "FLAT";
  positionSizeAtoms: bigint;
  absPos: number;
  entryPrice: number;
  costUsd: number;
  notional: number;
  margin: number;
  unrealizedPnl: number;
  pendingFunding: number;
  equity: number;
  leverage: number;
  maxLeverage: number;
  maintLeverage: number;
  liquidationPrice: number;
  distanceToLiq: number;
  maxNotional: number;
  maxPositionBase: number;
  oraclePrice: number;
  oracleMantissa: bigint;
  oracleExpo: number;
  initialMarginBps: number;
  maintenanceMarginBps: number;
  takerFeeBps: number;
  liquidationBufferBps: number;
  insuranceFundBalance: number;
  cumulativeFunding: bigint;
}

function computeMetrics(
  fixed: MarketFixed,
  seat: ClaimedSeat,
): PositionMetrics {
  const baseFactor = 10 ** fixed.baseMintDecimals;
  const quoteFactor = 10 ** fixed.quoteMintDecimals;

  const oraclePrice =
    Number(fixed.oraclePriceMantissa) * 10 ** fixed.oraclePriceExpo;
  const initialMarginBps = Number(fixed.initialMarginBps);
  const maintenanceMarginBps = Number(fixed.maintenanceMarginBps);
  const maxLeverage = 10_000 / initialMarginBps;
  const maintLeverage = 10_000 / maintenanceMarginBps;
  const takerFeeBps = Number(fixed.takerFeeBps);
  const liquidationBufferBps = Number(fixed.liquidationBufferBps);
  const insuranceFundBalance =
    Number(fixed.insuranceFundBalance) / quoteFactor;

  const positionSize = seat.positionSize;
  const isLong = positionSize > 0n;
  const isShort = positionSize < 0n;
  const direction = isLong ? "LONG" : isShort ? "SHORT" : "FLAT";

  const absPosBigint =
    positionSize >= 0n ? positionSize : -positionSize;
  const absPos = Number(absPosBigint) / baseFactor;
  const notional = absPos * oraclePrice;

  const margin = Number(seat.margin) / quoteFactor;
  const costUsd = Number(seat.costBasis) / quoteFactor;
  const entryPrice = positionSize !== 0n ? costUsd / absPos : 0;

  const currentValue = absPos * oraclePrice;
  const unrealizedPnl = isLong
    ? currentValue - costUsd
    : isShort
      ? costUsd - currentValue
      : 0;

  const equity = margin + unrealizedPnl;
  const leverage =
    equity > 0 && positionSize !== 0n ? notional / equity : 0;

  // Liquidation price
  const maintRatio = maintenanceMarginBps / 10_000;
  let liquidationPrice = 0;
  if (isLong) {
    liquidationPrice = (costUsd - margin) / (absPos * (1 - maintRatio));
  } else if (isShort) {
    liquidationPrice = (margin + costUsd) / (absPos * (1 + maintRatio));
  }
  const distanceToLiq =
    positionSize !== 0n
      ? Math.abs(((oraclePrice - liquidationPrice) / oraclePrice) * 100)
      : 0;

  // Max position
  const maxNotional = equity * maxLeverage;
  const maxPositionBase = oraclePrice > 0 ? maxNotional / oraclePrice : 0;

  // Pending funding
  const fundingDelta = fixed.cumulativeFunding - seat.lastCumulativeFunding;
  let pendingFunding = 0;
  if (positionSize !== 0n && fundingDelta !== 0n) {
    pendingFunding =
      Number((positionSize * fundingDelta) / FUNDING_SCALE) / quoteFactor;
  }

  return {
    direction,
    positionSizeAtoms: positionSize,
    absPos,
    entryPrice,
    costUsd,
    notional,
    margin,
    unrealizedPnl,
    pendingFunding,
    equity,
    leverage,
    maxLeverage,
    maintLeverage,
    liquidationPrice,
    distanceToLiq,
    maxNotional,
    maxPositionBase,
    oraclePrice,
    oracleMantissa: fixed.oraclePriceMantissa,
    oracleExpo: fixed.oraclePriceExpo,
    initialMarginBps: initialMarginBps,
    maintenanceMarginBps: maintenanceMarginBps,
    takerFeeBps,
    liquidationBufferBps,
    insuranceFundBalance,
    cumulativeFunding: fixed.cumulativeFunding,
  };
}

// ── Display ─────────────────────────────────────────────────────────────────
function printMetrics(
  marketKey: string,
  traderKey: string,
  m: PositionMetrics,
) {
  const marginAtoms = Math.round(m.margin * 10 ** 6); // approximate for display
  console.log("═══════════════════════════════════════════════════════");
  console.log(`  Market    : ${marketKey}`);
  console.log(`  Trader    : ${traderKey}`);
  console.log("═══════════════════════════════════════════════════════");
  console.log();
  console.log("── Oracle ─────────────────────────────────────────────");
  console.log(`  Price           : $${m.oraclePrice.toFixed(4)}`);
  console.log(`  Mantissa        : ${m.oracleMantissa}`);
  console.log(`  Exponent        : ${m.oracleExpo}`);
  console.log();
  console.log("── Position ───────────────────────────────────────────");
  console.log(`  Direction       : ${m.direction}`);
  console.log(
    `  Size            : ${m.absPos.toFixed(6)} base (${m.positionSizeAtoms} atoms)`,
  );
  console.log(`  Entry Price     : $${m.entryPrice.toFixed(4)}`);
  console.log(`  Cost Basis      : $${m.costUsd.toFixed(4)}`);
  console.log(`  Notional        : $${m.notional.toFixed(4)}`);
  console.log();
  console.log("── Margin & Equity ────────────────────────────────────");
  console.log(`  Margin (deposit): $${m.margin.toFixed(4)}`);
  console.log(
    `  Unrealized PnL  : $${m.unrealizedPnl >= 0 ? "+" : ""}${m.unrealizedPnl.toFixed(4)}`,
  );
  const fundingNote =
    m.pendingFunding > 0
      ? " (owed, will reduce equity)"
      : m.pendingFunding < 0
        ? " (receivable, will increase equity)"
        : "";
  console.log(
    `  Pending Funding : $${m.pendingFunding >= 0 ? "+" : ""}${m.pendingFunding.toFixed(4)}${fundingNote}`,
  );
  console.log(`  Equity          : $${m.equity.toFixed(4)}`);
  console.log();
  console.log("── Leverage & Liquidation ─────────────────────────────");
  console.log(`  Effective Leverage : ${m.leverage.toFixed(2)}x`);
  console.log(
    `  Max Leverage       : ${m.maxLeverage.toFixed(1)}x (initial margin ${m.initialMarginBps} bps = ${(m.initialMarginBps / 100).toFixed(0)}%)`,
  );
  console.log(
    `  Maint. Leverage    : ${m.maintLeverage.toFixed(1)}x (maintenance ${m.maintenanceMarginBps} bps = ${(m.maintenanceMarginBps / 100).toFixed(0)}%)`,
  );
  if (m.positionSizeAtoms !== 0n) {
    console.log(
      `  Liquidation Price  : $${m.liquidationPrice.toFixed(4)} (${m.distanceToLiq.toFixed(2)}% away)`,
    );
  } else {
    console.log("  Liquidation Price  : N/A (no position)");
  }
  console.log();
  console.log("── Max Position (at current equity) ───────────────────");
  console.log(`  Max Notional     : $${m.maxNotional.toFixed(2)}`);
  console.log(`  Max Size         : ${m.maxPositionBase.toFixed(6)} base`);
  console.log();
  console.log("── Market Parameters ──────────────────────────────────");
  console.log(
    `  Taker Fee        : ${m.takerFeeBps} bps (${(m.takerFeeBps / 100).toFixed(3)}%)`,
  );
  console.log(
    `  Liq. Buffer      : ${m.liquidationBufferBps} bps (${(m.liquidationBufferBps / 100).toFixed(1)}%)`,
  );
  console.log(
    `  Insurance Fund   : $${m.insuranceFundBalance.toFixed(4)}`,
  );
  console.log(
    `  Cumul. Funding   : ${m.cumulativeFunding} (scaled by 1e9)`,
  );
  console.log();
}

// ── Main ────────────────────────────────────────────────────────────────────
function parseArgs(): { rpc: string; market: string; trader: string } {
  const args = process.argv.slice(2);
  let rpc = "";
  let market = "";
  let trader = "";

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--rpc" && args[i + 1]) rpc = args[++i];
    else if (args[i] === "--market" && args[i + 1]) market = args[++i];
    else if (args[i] === "--trader" && args[i + 1]) trader = args[++i];
  }

  if (!rpc || !market || !trader) {
    console.error(
      "Usage: npx tsx scripts/position.ts --rpc <url> --market <pubkey> --trader <pubkey>",
    );
    process.exit(1);
  }
  return { rpc, market, trader };
}

async function main() {
  const { rpc, market, trader } = parseArgs();

  const connection = new Connection(rpc, "confirmed");
  const marketKey = new PublicKey(market);
  const traderKey = new PublicKey(trader);

  const account = await connection.getAccountInfo(marketKey);
  if (!account) {
    console.error("Market account not found");
    process.exit(1);
  }

  const data = Buffer.from(account.data);
  if (data.length < FIXED_HEADER_SIZE) {
    console.error(
      `Account data too small: ${data.length} bytes (need >= ${FIXED_HEADER_SIZE})`,
    );
    process.exit(1);
  }

  const fixed = deserializeMarketFixed(data);
  const dynamic = data.subarray(FIXED_HEADER_SIZE);

  const seat = findTraderSeat(
    dynamic,
    fixed.claimedSeatsRootIndex,
    traderKey,
  );
  if (!seat) {
    console.error(`Trader ${trader} not found on market ${market}`);
    process.exit(1);
  }

  const metrics = computeMetrics(fixed, seat);
  printMetrics(market, trader, metrics);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
