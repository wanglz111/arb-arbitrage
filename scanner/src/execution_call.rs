use std::sync::Arc;

use ethers::{
    providers::Middleware,
    types::{Address, Bytes, TransactionRequest, transaction::eip2718::TypedTransaction},
    utils::hex,
};

use crate::{config::ExecutionCallMode, execute::ExecutionPlan, state::RpcProvider};

#[derive(Clone)]
pub struct ExecutionCallSimulator {
    provider: Arc<RpcProvider>,
    executor: Address,
    caller: Address,
    mode: ExecutionCallMode,
}

#[derive(Clone, Debug)]
pub struct ExecutionCallReport {
    pub mode: Option<ExecutionCallMode>,
    pub success: bool,
    pub skipped_reason: Option<&'static str>,
    pub error: Option<String>,
    pub return_data: Option<Bytes>,
    pub calldata_bytes: usize,
}

impl ExecutionCallReport {
    pub fn skipped(reason: &'static str) -> Self {
        Self {
            mode: None,
            success: false,
            skipped_reason: Some(reason),
            error: None,
            return_data: None,
            calldata_bytes: 0,
        }
    }

    pub fn status(&self) -> &'static str {
        if self.success {
            "success"
        } else if self.skipped_reason.is_some() {
            "skipped"
        } else {
            "failed"
        }
    }

    pub fn return_data_hex(&self) -> Option<String> {
        self.return_data
            .as_ref()
            .map(|data| format!("0x{}", hex::encode(data.as_ref())))
    }
}

impl ExecutionCallSimulator {
    pub fn new(
        provider: Arc<RpcProvider>,
        executor: Address,
        caller: Address,
        mode: ExecutionCallMode,
    ) -> Self {
        Self {
            provider,
            executor,
            caller,
            mode,
        }
    }

    pub async fn simulate(&self, plan: &ExecutionPlan) -> ExecutionCallReport {
        let calldata = match self.mode {
            ExecutionCallMode::Direct => plan.execute_calldata.clone(),
            ExecutionCallMode::Route => plan.execute_route_calldata.clone(),
        };

        let request = TransactionRequest::new()
            .to(self.executor)
            .from(self.caller)
            .data(calldata.clone());
        let typed: TypedTransaction = request.into();

        match self.provider.call(&typed, None).await {
            Ok(return_data) => ExecutionCallReport {
                mode: Some(self.mode),
                success: true,
                skipped_reason: None,
                error: None,
                return_data: Some(return_data),
                calldata_bytes: calldata.len(),
            },
            Err(error) => ExecutionCallReport {
                mode: Some(self.mode),
                success: false,
                skipped_reason: None,
                error: Some(error.to_string()),
                return_data: None,
                calldata_bytes: calldata.len(),
            },
        }
    }
}
