const RPC_URL = process.env.HTTP_RPC_URL || "https://arb-mainnet.g.alchemy.com/v2/IYgELf7ICIKR_dTzwRWi4";
const LOG_RPC_URL = process.env.LOG_RPC_URL || "https://arb1.arbitrum.io/rpc";
const FACTORY = "0x1F98431c8aD98523631AE4a59f267346ea31F984";
const POOL_CREATED_TOPIC =
  "0x783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118";
const BLOCK_CHUNK = 10_000_000n;
const TOP_PAGES = 10;

async function rpc(method, params, url = RPC_URL) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  const json = await res.json();
  if (json.error) throw new Error(`${method}: ${JSON.stringify(json.error)}`);
  return json.result;
}

function hexToBigInt(hex) {
  return BigInt(hex);
}

function topicToAddress(topic) {
  return `0x${topic.slice(26)}`.toLowerCase();
}

function decodePoolCreated(log) {
  const token0 = topicToAddress(log.topics[1]);
  const token1 = topicToAddress(log.topics[2]);
  const fee = Number(hexToBigInt(log.topics[3]));
  const data = log.data.slice(2);
  const tickSpacingHex = `0x${data.slice(0, 64)}`;
  const poolHex = `0x${data.slice(64 + 24, 64 + 64)}`;
  let tickSpacing = Number(hexToBigInt(tickSpacingHex));
  if (tickSpacing >= 2 ** 23) tickSpacing -= 2 ** 24;
  return {
    token0,
    token1,
    fee,
    tickSpacing,
    pool: poolHex.toLowerCase(),
    blockNumber: Number(hexToBigInt(log.blockNumber)),
  };
}

async function getLogsRange(from, to) {
  try {
    return await rpc("eth_getLogs", [
      {
        address: FACTORY,
        fromBlock: `0x${from.toString(16)}`,
        toBlock: `0x${to.toString(16)}`,
        topics: [POOL_CREATED_TOPIC],
      },
    ], LOG_RPC_URL);
  } catch (error) {
    if (from === to) throw error;
    const message = String(error.message || error);
    if (!message.includes("timed out") && !message.includes("limit")) throw error;
    const mid = (from + to) / 2n;
    const left = await getLogsRange(from, mid);
    const right = await getLogsRange(mid + 1n, to);
    return left.concat(right);
  }
}

async function scanFactory() {
  const latest = hexToBigInt(await rpc("eth_blockNumber", [], LOG_RPC_URL));
  const feeCounts = new Map();
  const poolMap = new Map();
  let from = 0n;
  while (from <= latest) {
    const to = from + BLOCK_CHUNK > latest ? latest : from + BLOCK_CHUNK;
    const logs = await getLogsRange(from, to);
    for (const log of logs) {
      const decoded = decodePoolCreated(log);
      poolMap.set(decoded.pool, decoded);
      feeCounts.set(decoded.fee, (feeCounts.get(decoded.fee) || 0) + 1);
    }
    from = to + 1n;
  }
  return { latest: Number(latest), feeCounts, poolMap };
}

async function fetchJson(url) {
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const res = await fetch(url, {
      headers: {
        accept: "application/json",
        "user-agent": "arb-arbitrage-research/0.1",
      },
    });
    if (res.ok) return res.json();
    if (res.status !== 429 || attempt === 5) {
      throw new Error(`${url}: ${res.status}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 1500 * (attempt + 1)));
  }
}

async function fetchTopPools() {
  const all = [];
  for (let page = 1; page <= TOP_PAGES; page += 1) {
    const url =
      `https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/uniswap_v3_arbitrum/pools` +
      `?page=${page}&sort=h24_volume_usd_desc&include=base_token,quote_token`;
    const json = await fetchJson(url);
    const included = new Map((json.included || []).map((x) => [x.id, x]));
    for (const pool of json.data || []) {
      const baseId = pool.relationships?.base_token?.data?.id;
      const quoteId = pool.relationships?.quote_token?.data?.id;
      const base = included.get(baseId);
      const quote = included.get(quoteId);
      all.push({
        address: pool.attributes.address.toLowerCase(),
        name: pool.attributes.name,
        reserveUsd: Number(pool.attributes.reserve_in_usd || 0),
        volume24h: Number(pool.attributes.volume_usd?.h24 || 0),
        createdAt: pool.attributes.pool_created_at,
        baseSymbol: base?.attributes?.symbol || base?.attributes?.name || "UNKNOWN",
        quoteSymbol: quote?.attributes?.symbol || quote?.attributes?.name || "UNKNOWN",
        baseAddress: base?.attributes?.address?.toLowerCase() || null,
        quoteAddress: quote?.attributes?.address?.toLowerCase() || null,
      });
    }
  }
  return all;
}

function summarizeTopPools(pools, poolMap) {
  const tokenStats = new Map();
  const enriched = [];
  for (const pool of pools) {
    const chainMeta = poolMap.get(pool.address);
    const fee = chainMeta?.fee ?? null;
    enriched.push({ ...pool, fee });
    for (const [addr, symbol] of [
      [pool.baseAddress, pool.baseSymbol],
      [pool.quoteAddress, pool.quoteSymbol],
    ]) {
      if (!addr) continue;
      const prev = tokenStats.get(addr) || { symbol, pools: 0, reserveUsd: 0, volume24h: 0 };
      prev.pools += 1;
      prev.reserveUsd += pool.reserveUsd;
      prev.volume24h += pool.volume24h;
      tokenStats.set(addr, prev);
    }
  }
  enriched.sort((a, b) => b.reserveUsd - a.reserveUsd);
  const topTokens = [...tokenStats.entries()]
    .map(([address, value]) => ({ address, ...value }))
    .sort((a, b) => b.reserveUsd - a.reserveUsd)
    .slice(0, 20);
  return { enriched, topTokens };
}

function printSummary(factorySummary, topSummary) {
  const feeCounts = [...factorySummary.feeCounts.entries()].sort((a, b) => a[0] - b[0]);
  const totalPools = [...factorySummary.feeCounts.values()].reduce((a, b) => a + b, 0);
  console.log(JSON.stringify({
    latestBlock: factorySummary.latest,
    totalPools,
    feeCounts: Object.fromEntries(feeCounts.map(([fee, count]) => [String(fee), count])),
    topPoolsByReserve: topSummary.enriched.slice(0, 25),
    topTokensByReservePresence: topSummary.topTokens,
  }, null, 2));
}

const factorySummary = await scanFactory();
const topPools = await fetchTopPools();
const topSummary = summarizeTopPools(topPools, factorySummary.poolMap);
printSummary(factorySummary, topSummary);
