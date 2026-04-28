// SPDX-License-Identifier: MIT
pragma solidity ^0.8.26;

import {RouteArb} from "./RouteArb.sol";

contract TriangleArb is RouteArb {
    constructor(address balancerVault_, address swapRouter_) RouteArb(balancerVault_, swapRouter_, msg.sender) {}
}
