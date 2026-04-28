// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {RouteArb} from "./RouteArb.sol";

contract TriangleArb is RouteArb {
    constructor(address morpho_, address swapRouter_) RouteArb(morpho_, swapRouter_, msg.sender) {}
}
