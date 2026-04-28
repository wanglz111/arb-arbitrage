// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

interface IERC20 {
    function balanceOf(address account) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
}

interface IMorpho {
    function flashLoan(address token, uint256 assets, bytes calldata data) external;
}

interface IMorphoFlashLoanCallback {
    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external;
}

contract MorphoFlashLoanTest is IMorphoFlashLoanCallback {
    address internal constant MORPHO = 0x6c247b1F6182318877311737BaC0844bAa518F5e;

    address internal constant USDC = 0xaf88d065e77c8cC2239327C5EDb3A432268e5831;
    address internal constant USDT0 = 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9;
    address internal constant WETH = 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1;
    address internal constant WBTC = 0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f;
    address internal constant WSTETH = 0x5979D7b546E38E414F7E9822514be443A4800529;
    address internal constant WEETH = 0x35751007a407ca6FEFfE80b3cB397736D2cf4dbe;

    event FlashLoanProbe(string symbol, address token, uint256 available, uint256 borrowed);

    struct ProbeState {
        address token;
        uint256 amount;
        bool seen;
    }

    ProbeState internal probe;

    function testFlashLoanMajorTokens() external {
        if (MORPHO.code.length == 0) return;

        _assertFlashLoan("USDC", USDC, 1_000_000e6);
        _assertFlashLoan("USDT0", USDT0, 1_000_000e6);
        _assertFlashLoan("WETH", WETH, 100 ether);
        _assertFlashLoan("WBTC", WBTC, 10e8);
        _assertFlashLoan("wstETH", WSTETH, 250 ether);
        _assertFlashLoan("weETH", WEETH, 250 ether);
    }

    function testFlashLoanRevertsAboveAvailable() external {
        if (MORPHO.code.length == 0) return;

        uint256 available = IERC20(USDC).balanceOf(MORPHO);
        require(available > 0, "USDC unavailable");

        (bool ok,) = MORPHO.call(
            abi.encodeWithSelector(IMorpho.flashLoan.selector, USDC, available + 1, abi.encode(USDC, available + 1))
        );

        require(!ok, "flash loan above balance should revert");
    }

    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external override {
        require(msg.sender == MORPHO, "unexpected callback caller");

        (address token, uint256 expectedAssets) = abi.decode(data, (address, uint256));
        require(token == probe.token, "unexpected token");
        require(expectedAssets == probe.amount, "unexpected encoded amount");
        require(assets == probe.amount, "unexpected callback amount");
        require(IERC20(token).balanceOf(address(this)) >= assets, "loaned assets not received");

        probe.seen = true;

        require(IERC20(token).approve(MORPHO, 0), "approve reset failed");
        require(IERC20(token).approve(MORPHO, assets), "approve repayment failed");
    }

    function _assertFlashLoan(string memory symbol, address token, uint256 cap) internal {
        uint256 available = IERC20(token).balanceOf(MORPHO);
        require(available > 0, "token unavailable");

        uint256 amount = available / 3;
        if (amount > cap) amount = cap;
        if (amount == 0) amount = available;

        uint256 morphoBefore = IERC20(token).balanceOf(MORPHO);
        uint256 selfBefore = IERC20(token).balanceOf(address(this));

        probe = ProbeState({token: token, amount: amount, seen: false});

        emit FlashLoanProbe(symbol, token, available, amount);
        IMorpho(MORPHO).flashLoan(token, amount, abi.encode(token, amount));

        require(probe.seen, "callback not executed");
        require(IERC20(token).balanceOf(address(this)) == selfBefore, "test contract retained funds");
        require(IERC20(token).balanceOf(MORPHO) == morphoBefore, "morpho balance not restored");
    }
}
