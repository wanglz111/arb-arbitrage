// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

interface IERC20RouteMinimal {
    function approve(address spender, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

interface IMorphoRouteMinimal {
    function flashLoan(address token, uint256 assets, bytes calldata data) external;
}

interface IMorphoFlashLoanCallbackRouteMinimal {
    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external;
}

interface ISwapRouterV3Route {
    struct ExactInputParams {
        bytes path;
        address recipient;
        uint256 deadline;
        uint256 amountIn;
        uint256 amountOutMinimum;
    }

    function exactInput(ExactInputParams calldata params) external payable returns (uint256 amountOut);
}

contract RouteArb is IMorphoFlashLoanCallbackRouteMinimal {
    uint256 internal constant V3_FIRST_TOKEN_BYTES = 20;
    uint256 internal constant V3_NEXT_HOP_BYTES = 23;
    uint256 internal constant MIN_ROUTE_HOPS = 3;
    uint256 internal constant MAX_ROUTE_HOPS = 5;

    address public immutable morpho;
    ISwapRouterV3Route public immutable swapRouter;

    address public owner;
    address public profitRecipient;
    uint256 public lastAmountOut;

    mapping(address => bool) public tokenApprovalsSet;
    mapping(uint256 => bytes) public routes;

    event OwnerUpdated(address indexed previousOwner, address indexed nextOwner);
    event ProfitRecipientUpdated(address indexed previousRecipient, address indexed nextRecipient);
    event RouteUpdated(uint256 indexed routeId, bytes path);
    event FlashExecution(
        address indexed loanToken,
        uint256 indexed routeId,
        uint256 loanAmount,
        uint256 amountOut,
        uint256 profit
    );

    modifier onlyOwner() {
        require(msg.sender == owner, "only owner");
        _;
    }

    constructor(address morpho_, address swapRouter_, address profitRecipient_) {
        require(morpho_ != address(0), "invalid morpho");
        require(swapRouter_ != address(0), "invalid router");
        require(profitRecipient_ != address(0), "invalid recipient");

        morpho = morpho_;
        swapRouter = ISwapRouterV3Route(swapRouter_);
        owner = msg.sender;
        profitRecipient = profitRecipient_;
    }

    function setOwner(address nextOwner) external onlyOwner {
        require(nextOwner != address(0), "invalid owner");
        emit OwnerUpdated(owner, nextOwner);
        owner = nextOwner;
    }

    function setProfitRecipient(address nextRecipient) external onlyOwner {
        require(nextRecipient != address(0), "invalid recipient");
        emit ProfitRecipientUpdated(profitRecipient, nextRecipient);
        profitRecipient = nextRecipient;
    }

    function setRoute(uint256 routeId, bytes calldata path) external onlyOwner {
        _validateClosedV3PathCalldata(path);
        routes[routeId] = path;
        emit RouteUpdated(routeId, path);
    }

    function execute(uint256 loanAmount, uint256 amountOutMinimum, bytes calldata path) external onlyOwner {
        _executeCalldata(0, loanAmount, amountOutMinimum, path);
    }

    function executeRoute(uint256 routeId, uint256 loanAmount, uint256 amountOutMinimum) external onlyOwner {
        bytes memory path = routes[routeId];
        require(path.length != 0, "unknown route");
        _executeMemory(routeId, loanAmount, amountOutMinimum, path);
    }

    function onMorphoFlashLoan(uint256 assets, bytes calldata data) external override {
        require(msg.sender == morpho, "unexpected morpho callback");

        (uint256 routeId, uint256 amountOutMinimum, bytes memory path) = abi.decode(data, (uint256, uint256, bytes));
        _validateClosedV3PathMemory(path);

        address loanToken = _firstTokenFromMemory(path);
        _ensureApprovals(loanToken);

        uint256 amountOut = swapRouter.exactInput(
            ISwapRouterV3Route.ExactInputParams({
                path: path,
                recipient: address(this),
                deadline: block.timestamp,
                amountIn: assets,
                amountOutMinimum: amountOutMinimum
            })
        );

        lastAmountOut = amountOut;

        require(amountOut >= assets, "insufficient repayment");

        uint256 profit = amountOut - assets;
        if (profit != 0) {
            require(IERC20RouteMinimal(loanToken).transfer(profitRecipient, profit), "profit transfer failed");
        }

        emit FlashExecution(loanToken, routeId, assets, amountOut, profit);
    }

    function _executeMemory(uint256 routeId, uint256 loanAmount, uint256 amountOutMinimum, bytes memory path) internal {
        _validateClosedV3PathMemory(path);
        address loanToken = _firstTokenFromMemory(path);
        _startFlashLoan(loanToken, loanAmount, abi.encode(routeId, amountOutMinimum, path));
    }

    function _executeCalldata(uint256 routeId, uint256 loanAmount, uint256 amountOutMinimum, bytes calldata path)
        internal
    {
        _validateClosedV3PathCalldata(path);
        address loanToken = _firstTokenFromCalldata(path);
        _startFlashLoan(loanToken, loanAmount, abi.encode(routeId, amountOutMinimum, path));
    }

    function _startFlashLoan(address loanToken, uint256 loanAmount, bytes memory data) internal {
        IMorphoRouteMinimal(morpho).flashLoan(loanToken, loanAmount, data);
    }

    function _ensureApprovals(address token) internal {
        if (tokenApprovalsSet[token]) {
            return;
        }

        require(IERC20RouteMinimal(token).approve(address(swapRouter), type(uint256).max), "router approve failed");
        require(IERC20RouteMinimal(token).approve(morpho, type(uint256).max), "morpho approve failed");
        tokenApprovalsSet[token] = true;
    }

    function _validateClosedV3PathCalldata(bytes calldata path) internal pure {
        uint256 hops = _validateV3PathLength(path.length);
        address first = _firstTokenFromCalldata(path);
        address last;
        assembly {
            last := shr(96, calldataload(add(path.offset, sub(path.length, 20))))
        }
        require(first == last, "path must close loop");
        require(hops >= MIN_ROUTE_HOPS && hops <= MAX_ROUTE_HOPS, "invalid hop count");
    }

    function _validateClosedV3PathMemory(bytes memory path) internal pure {
        uint256 hops = _validateV3PathLength(path.length);
        address first = _firstTokenFromMemory(path);
        address last;
        assembly {
            last := shr(96, mload(add(add(path, 32), sub(mload(path), 20))))
        }
        require(first == last, "path must close loop");
        require(hops >= MIN_ROUTE_HOPS && hops <= MAX_ROUTE_HOPS, "invalid hop count");
    }

    function _validateV3PathLength(uint256 pathLength) internal pure returns (uint256 hops) {
        require(pathLength > V3_FIRST_TOKEN_BYTES, "invalid path");
        uint256 remainder = pathLength - V3_FIRST_TOKEN_BYTES;
        require(remainder % V3_NEXT_HOP_BYTES == 0, "invalid path");
        hops = remainder / V3_NEXT_HOP_BYTES;
    }

    function _firstTokenFromCalldata(bytes calldata path) internal pure returns (address token) {
        assembly {
            token := shr(96, calldataload(path.offset))
        }
    }

    function _firstTokenFromMemory(bytes memory path) internal pure returns (address token) {
        assembly {
            token := shr(96, mload(add(path, 32)))
        }
    }
}
