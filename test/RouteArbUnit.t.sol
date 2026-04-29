// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {RouteArb} from "../src/RouteArb.sol";

interface VmRouteUnit {
    function prank(address msgSender) external;
}

contract MockRouteToken {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        return true;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        require(balanceOf[from] >= amount, "balance");
        uint256 allowed = allowance[from][msg.sender];
        require(allowed >= amount, "allowance");
        if (allowed != type(uint256).max) {
            allowance[from][msg.sender] = allowed - amount;
        }
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        return true;
    }
}

contract MockMorphoRoute {
    function flashLoan(address tokenAddress, uint256 assets, bytes calldata data) external {
        MockRouteToken token = MockRouteToken(tokenAddress);
        uint256 balanceBefore = token.balanceOf(address(this));
        require(token.transfer(msg.sender, assets), "loan transfer failed");

        IMorphoFlashLoanCallbackRouteUnit(msg.sender).onMorphoFlashLoan(assets, data);

        require(token.transferFrom(msg.sender, address(this), assets), "repayment failed");
        require(token.balanceOf(address(this)) >= balanceBefore, "not repaid");
    }
}

interface IMorphoFlashLoanCallbackRouteUnit {
    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external;
}

contract MockSwapRouterRoute {
    struct ExactInputParams {
        bytes path;
        address recipient;
        uint256 deadline;
        uint256 amountIn;
        uint256 amountOutMinimum;
    }

    uint256 public bonus;

    constructor(uint256 bonus_) {
        bonus = bonus_;
    }

    function exactInput(ExactInputParams calldata params) external returns (uint256 amountOut) {
        address token = _firstToken(params.path);
        amountOut = params.amountIn + bonus;
        require(MockRouteToken(token).transferFrom(msg.sender, address(this), params.amountIn), "pull failed");
        MockRouteToken(token).mint(params.recipient, amountOut);
        require(amountOut >= params.amountOutMinimum, "too little out");
    }

    function _firstToken(bytes calldata path) internal pure returns (address token) {
        assembly {
            token := shr(96, calldataload(path.offset))
        }
    }
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

    function testExecuteUsesMorphoAndPaysProfit() external {
        MockRouteToken token = new MockRouteToken();
        MockMorphoRoute morpho = new MockMorphoRoute();
        MockSwapRouterRoute router = new MockSwapRouterRoute(1);
        RouteArb routeArb = new RouteArb(address(morpho), address(router), address(this));
        bytes memory path = abi.encodePacked(address(token), FEE_500, USDT0, FEE_100, USDC, FEE_500, address(token));

        token.mint(address(morpho), 1_000);

        routeArb.execute(100, 0, path);

        require(token.balanceOf(address(morpho)) == 1_000, "morpho not repaid");
        require(token.balanceOf(address(this)) == 1, "profit not paid");
        require(token.balanceOf(address(routeArb)) == 0, "executor retained token");
        require(routeArb.lastAmountOut() == 101, "last amount out");
    }
}
