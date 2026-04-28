// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {RouteArb} from "../src/RouteArb.sol";

interface VmRouteUnit {
    function prank(address msgSender) external;
}

contract RouteArbUnitTest {
    address internal constant VM_ADDRESS = address(uint160(uint256(keccak256("hevm cheat code"))));
    VmRouteUnit internal constant vm = VmRouteUnit(VM_ADDRESS);

    address internal constant MORPHO = 0x0000000000000000000000000000000000001000;
    address internal constant SWAP_ROUTER = 0x0000000000000000000000000000000000002000;
    address internal constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address internal constant USDT0 = 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9;
    address internal constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address internal constant WBTC = 0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f;
    address internal constant CBBTC = 0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf;

    uint24 internal constant FEE_100 = 100;
    uint24 internal constant FEE_500 = 500;

    RouteArb internal arb;

    function setUp() public {
        arb = new RouteArb(MORPHO, SWAP_ROUTER, address(this));
    }

    function testSetRouteAcceptsFourHopClosedPath() external {
        bytes memory path = abi.encodePacked(WBTC, FEE_500, USDT0, FEE_100, USDC, FEE_500, CBBTC, FEE_500, WBTC);

        arb.setRoute(7, path);

        require(keccak256(arb.routes(7)) == keccak256(path), "stored route mismatch");
    }

    function testSetRouteRejectsOpenPath() external {
        bytes memory path = abi.encodePacked(WBTC, FEE_500, USDT0, FEE_100, USDC, FEE_500, CBBTC);

        (bool ok,) = address(arb).call(abi.encodeWithSelector(RouteArb.setRoute.selector, 1, path));
        require(!ok, "open path accepted");
    }

    function testSetRouteRejectsTwoHopPath() external {
        bytes memory path = abi.encodePacked(USDC, FEE_500, WETH, FEE_500, USDC);

        (bool ok,) = address(arb).call(abi.encodeWithSelector(RouteArb.setRoute.selector, 1, path));
        require(!ok, "two-hop path accepted");
    }

    function testSetRouteIsOwnerOnly() external {
        bytes memory path = abi.encodePacked(USDC, FEE_500, WETH, FEE_500, WBTC, FEE_500, USDC);

        vm.prank(address(0xBEEF));
        (bool ok,) = address(arb).call(abi.encodeWithSelector(RouteArb.setRoute.selector, 1, path));
        require(!ok, "non-owner set route");
    }
}
