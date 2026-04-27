// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

interface IERC20Minimal {
    function approve(address spender, uint256 amount) external returns (bool);
}

interface IMorphoMinimal {
    function flashLoan(address token, uint256 assets, bytes calldata data) external;
}

interface IMorphoFlashLoanCallbackMinimal {
    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external;
}

interface ISwapRouterV3 {
    struct ExactInputParams {
        bytes path;
        address recipient;
        uint256 deadline;
        uint256 amountIn;
        uint256 amountOutMinimum;
    }

    function exactInput(ExactInputParams calldata params) external payable returns (uint256 amountOut);
}

contract TriangleArb is IMorphoFlashLoanCallbackMinimal {
    uint256 internal constant TRIANGLE_PATH_BYTES_LENGTH = 89;

    address public immutable morpho;
    ISwapRouterV3 public immutable swapRouter;

    uint256 public lastAmountOut;
    mapping(address => bool) public tokenApprovalsSet;

    event FlashExecution(address indexed loanToken, uint256 loanAmount, uint256 amountOut);

    constructor(address morpho_, address swapRouter_) {
        morpho = morpho_;
        swapRouter = ISwapRouterV3(swapRouter_);
    }

    function execute(uint256 loanAmount, uint256 amountOutMinimum, bytes calldata path) external {
        address loanToken = _firstTokenFromCalldata(path);
        require(loanToken == _lastTokenFromCalldata(path), "path must close loop");

        IMorphoMinimal(morpho).flashLoan(loanToken, loanAmount, abi.encode(amountOutMinimum, path));
    }

    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external override {
        require(msg.sender == morpho, "unexpected morpho callback");

        (uint256 amountOutMinimum, bytes memory path) = abi.decode(data, (uint256, bytes));
        address loanToken = _firstTokenFromMemory(path);
        require(path.length == TRIANGLE_PATH_BYTES_LENGTH, "invalid triangle path");
        require(loanToken == _lastTokenFromMemory(path), "path must close loop");

        _ensureApprovals(loanToken);

        uint256 amountOut = swapRouter.exactInput(
            ISwapRouterV3.ExactInputParams({
                path: path,
                recipient: address(this),
                deadline: block.timestamp,
                amountIn: assets,
                amountOutMinimum: amountOutMinimum
            })
        );

        lastAmountOut = amountOut;

        emit FlashExecution(loanToken, assets, amountOut);
    }

    function _ensureApprovals(address token) internal {
        if (tokenApprovalsSet[token]) {
            return;
        }

        require(IERC20Minimal(token).approve(address(swapRouter), type(uint256).max), "router approve failed");
        require(IERC20Minimal(token).approve(morpho, type(uint256).max), "morpho approve failed");
        tokenApprovalsSet[token] = true;
    }

    function _firstTokenFromCalldata(bytes calldata path) internal pure returns (address token) {
        require(path.length == TRIANGLE_PATH_BYTES_LENGTH, "invalid triangle path");
        assembly {
            token := shr(96, calldataload(path.offset))
        }
    }

    function _lastTokenFromCalldata(bytes calldata path) internal pure returns (address token) {
        require(path.length == TRIANGLE_PATH_BYTES_LENGTH, "invalid triangle path");
        assembly {
            token := shr(96, calldataload(add(path.offset, 69)))
        }
    }

    function _firstTokenFromMemory(bytes memory path) internal pure returns (address token) {
        require(path.length == TRIANGLE_PATH_BYTES_LENGTH, "invalid triangle path");
        assembly {
            token := shr(96, mload(add(path, 32)))
        }
    }

    function _lastTokenFromMemory(bytes memory path) internal pure returns (address token) {
        require(path.length == TRIANGLE_PATH_BYTES_LENGTH, "invalid triangle path");
        assembly {
            token := shr(96, mload(add(path, 101)))
        }
    }
}
