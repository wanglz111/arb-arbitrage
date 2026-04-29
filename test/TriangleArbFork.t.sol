// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {TriangleArb} from "../src/TriangleArb.sol";

interface IERC20Fork {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 amount) external returns (bool);
}

interface IQuoterV2 {
    function quoteExactInput(bytes memory path, uint256 amountIn)
        external
        returns (uint256 amountOut, uint160[] memory, uint32[] memory, uint256 gasEstimate);
}

interface Vm {
    function prank(address msgSender) external;
}

contract TriangleArbForkTest {
    address internal constant VM_ADDRESS = address(uint160(uint256(keccak256("hevm cheat code"))));
    Vm internal constant vm = Vm(VM_ADDRESS);

    address internal constant MORPHO = 0x6c247b1F6182318877311737BaC0844bAa518F5e;
    address internal constant QUOTER_V2 = 0x61fFE014bA17989E743c5F6cB21bF9697530B21e;
    address internal constant SWAP_ROUTER = 0xE592427A0AEce92De3Edee1F18E0157C05861564;

    address internal constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address internal constant USDT0 = 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9;
    address internal constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address internal constant WBTC = 0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f;
    address internal constant ARB = 0x912CE59144191C1204E64559FE8253a0e49E6548;

    uint24 internal constant FEE_100 = 100;
    uint24 internal constant FEE_500 = 500;
    uint24 internal constant FEE_3000 = 3000;

    TriangleArb internal arb;

    function setUp() public {
        arb = new TriangleArb(MORPHO, SWAP_ROUTER);
    }

    function testSmokeExecuteCoreTriangles() external {
        if (MORPHO.code.length == 0 || QUOTER_V2.code.length == 0 || SWAP_ROUTER.code.length == 0) return;

        _smoke(
            "USDC-WETH-WBTC-USDC", USDC, 50_000e6, abi.encodePacked(USDC, FEE_500, WETH, FEE_500, WBTC, FEE_500, USDC)
        );

        _smoke(
            "USDC-WETH-USDT0-USDC", USDC, 50_000e6, abi.encodePacked(USDC, FEE_500, WETH, FEE_500, USDT0, FEE_100, USDC)
        );

        _smoke(
            "USDT0-WETH-WBTC-USDT0",
            USDT0,
            50_000e6,
            abi.encodePacked(USDT0, FEE_500, WETH, FEE_500, WBTC, FEE_500, USDT0)
        );

        _smoke(
            "USDC-WETH-ARB-USDC", USDC, 20_000e6, abi.encodePacked(USDC, FEE_500, WETH, FEE_500, ARB, FEE_3000, USDC)
        );
    }

    function _smoke(string memory label, address loanToken, uint256 loanAmount, bytes memory path) internal {
        uint256 morphoStart = IERC20Fork(loanToken).balanceOf(MORPHO);
        require(morphoStart > loanAmount, "insufficient morpho balance");

        (uint256 quotedOut,,,) = IQuoterV2(QUOTER_V2).quoteExactInput(path, loanAmount);
        uint256 topup = 0;

        if (quotedOut < loanAmount) {
            topup = (loanAmount - quotedOut) + 1;
            require(morphoStart > loanAmount + topup, "insufficient morpho topup balance");
            vm.prank(MORPHO);
            require(IERC20Fork(loanToken).transfer(address(arb), topup), "topup transfer failed");
        }

        arb.execute(loanAmount, 0, path);

        require(IERC20Fork(loanToken).balanceOf(MORPHO) >= morphoStart - topup, "morpho not repaid");
        require(arb.lastAmountOut() > 0, label);
    }
}
