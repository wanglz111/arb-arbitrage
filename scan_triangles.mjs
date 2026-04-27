import { execFileSync } from "node:child_process";

const RPC_URL = process.env.HTTP_RPC_URL || "https://arb-mainnet.g.alchemy.com/v2/IYgELf7ICIKR_dTzwRWi4";
const QUOTER_V2 = "0x61fFE014bA17989E743c5F6cB21bF9697530B21e";

const TOKENS = {
  USDC: { address: "0xaf88d065e77c8cc2239327c5edb3a432268e5831", decimals: 6 },
  USDT0: { address: "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9", decimals: 6 },
  WETH: { address: "0x82af49447d8a07e3bd95bd0d56f35241523fbab1", decimals: 18 },
  WBTC: { address: "0x2f2a2543b76a4166549f7aab2e75bef0aefc5b0f", decimals: 8 },
  ARB: { address: "0x912ce59144191c1204e64559fe8253a0e49e6548", decimals: 18 },
};

const ROUTES = [
  {
    name: "USDC-WETH-WBTC-USDC",
    start: "USDC",
    hops: [
      ["USDC", 500],
      ["WETH", 500],
      ["WBTC", 500],
      ["USDC"],
    ],
    sizes: [1_000, 5_000, 10_000, 50_000],
  },
  {
    name: "USDC-WETH-USDT0-USDC",
    start: "USDC",
    hops: [
      ["USDC", 500],
      ["WETH", 500],
      ["USDT0", 100],
      ["USDC"],
    ],
    sizes: [1_000, 5_000, 10_000, 50_000],
  },
  {
    name: "USDT0-WETH-WBTC-USDT0",
    start: "USDT0",
    hops: [
      ["USDT0", 500],
      ["WETH", 500],
      ["WBTC", 500],
      ["USDT0"],
    ],
    sizes: [1_000, 5_000, 10_000, 50_000],
  },
  {
    name: "USDC-WETH-ARB-USDC",
    start: "USDC",
    hops: [
      ["USDC", 500],
      ["WETH", 500],
      ["ARB", 3000],
      ["USDC"],
    ],
    sizes: [1_000, 5_000, 10_000, 20_000],
  },
];

function encodePath(hops) {
  let hex = "0x";
  for (let i = 0; i < hops.length - 1; i += 1) {
    const [symbol, fee] = hops[i];
    const nextSymbol = hops[i + 1][0];
    hex += TOKENS[symbol].address.slice(2);
    hex += fee.toString(16).padStart(6, "0");
    if (i === hops.length - 2) hex += TOKENS[nextSymbol].address.slice(2);
  }
  return hex.toLowerCase();
}

function parseUnits(value, decimals) {
  return BigInt(Math.round(value * 10 ** Math.min(decimals, 6))) * 10n ** BigInt(decimals - Math.min(decimals, 6));
}

function formatUnits(raw, decimals) {
  return Number(raw) / 10 ** decimals;
}

function parseCastScalar(line) {
  return line.split(" ")[0].trim();
}

async function quote(path, amountIn) {
  const out = execFileSync(
    "cast",
    [
      "call",
      QUOTER_V2,
      "quoteExactInput(bytes,uint256)(uint256,uint160[],uint32[],uint256)",
      path,
      amountIn.toString(),
      "--rpc-url",
      RPC_URL,
    ],
    { encoding: "utf8" }
  ).trim();
  const [amountOutRaw, , , gasEstimateRaw] = out
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  return {
    amountOut: BigInt(parseCastScalar(amountOutRaw)),
    gasEstimate: BigInt(parseCastScalar(gasEstimateRaw)),
  };
}

const rows = [];
for (const route of ROUTES) {
  const path = encodePath(route.hops);
  const decimals = TOKENS[route.start].decimals;
  for (const size of route.sizes) {
    const amountIn = parseUnits(size, decimals);
    const { amountOut, gasEstimate } = await quote(path, amountIn);
    const inFloat = formatUnits(amountIn, decimals);
    const outFloat = formatUnits(amountOut, decimals);
    rows.push({
      route: route.name,
      amountIn: inFloat,
      amountOut: outFloat,
      grossPnl: outFloat - inFloat,
      grossBps: ((outFloat - inFloat) / inFloat) * 10_000,
      gasEstimate: Number(gasEstimate),
    });
  }
}

rows.sort((a, b) => b.grossPnl - a.grossPnl);
console.log(JSON.stringify(rows, null, 2));
