use super::{Menu, AddressInput, AmountInput, ConfirmInput, NumberInput, SigningBoxInput, Terminal, UserInfo};
use super::echo::Echo;
use super::stdout::Stdout;
use crate::debot::{ManifestProcessor, ProcessorError};
use crate::config::Config;
use crate::helpers::TonClient;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use ton_client::debot::{DebotInterface, DebotInterfaceExecutor, InterfaceResult};
use ton_client::encoding::{decode_abi_number, decode_abi_bigint};
use ton_client::abi::Abi;
use num_traits::cast::NumCast;
use num_bigint::BigInt;
use std::sync::RwLock;

pub struct SupportedInterfaces {
    client: TonClient,
    interfaces: HashMap<String, Arc<dyn DebotInterface + Send + Sync>>,
}

#[async_trait::async_trait]
impl DebotInterfaceExecutor for SupportedInterfaces {
    fn get_interfaces<'a>(&'a self) -> &'a HashMap<String, Arc<dyn DebotInterface + Send + Sync>> {
        &self.interfaces
    }
    fn get_client(&self) -> TonClient {
        self.client.clone()
    }
}

struct InterfaceWrapper {
    processor: Arc<RwLock<ManifestProcessor>>,
}
impl InterfaceWrapper {
    fn wrap(
        &self,
        iface: Arc<dyn DebotInterface + Send + Sync>,
    ) -> Arc<dyn DebotInterface + Send + Sync> {
        Arc::new(BrowserInterface::new(iface, self.processor.clone()))
    }
}

impl SupportedInterfaces {
    pub fn new(client: TonClient, conf: &Config, proc: ManifestProcessor) -> Self {
        let mut interfaces = HashMap::new();

        let iw = InterfaceWrapper { processor: Arc::new(RwLock::new(proc)) };
        
        let iface: Arc<dyn DebotInterface + Send + Sync> = iw.wrap(Arc::new(AddressInput::new(conf.clone())));
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = iw.wrap(Arc::new(AmountInput::new()));
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(NumberInput::new());
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(ConfirmInput::new());
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(Stdout::new());
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(Echo::new());
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(Terminal::new());
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(Menu::new());
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(SigningBoxInput::new(client.clone()));
        interfaces.insert(iface.get_id(), iface);

        let iface: Arc<dyn DebotInterface + Send + Sync> = Arc::new(UserInfo::new(conf.clone()));
        interfaces.insert(iface.get_id(), iface);

        Self { client, interfaces }
    }
}

struct BrowserInterface {
    processor: Arc<RwLock<ManifestProcessor>>,
    inner_interface: Arc<dyn DebotInterface + Send + Sync>,
}

impl BrowserInterface {
    fn new(inner_interface: Arc<dyn DebotInterface + Send + Sync>, processor: Arc<RwLock<ManifestProcessor>>) -> Self {
        Self { inner_interface, processor}
    }
}

#[async_trait::async_trait]
impl DebotInterface for BrowserInterface {
    fn get_id(&self) -> String {
        self.inner_interface.get_id()
    }

    fn get_abi(&self) -> Abi {
        self.inner_interface.get_abi()
    }

    async fn call(&self, func: &str, args: &Value) -> InterfaceResult {
        let result = self.processor.write().unwrap().next_input(&self.get_id(), func, args);
        match result {
            Err(ProcessorError::InterfaceCallNeeded) => self.inner_interface.call(func, args).await,
            Err(e) => Err(format!("{:?}", e))?,
            Ok(params) => {
                let answer_id = decode_answer_id(args)?;
                Ok( (answer_id, params.unwrap_or(json!({})) ) )
            }
        }
        
    }
}

pub fn decode_answer_id(args: &Value) -> Result<u32, String> {
    u32::from_str_radix(
        args["answerId"]
            .as_str()
            .ok_or(format!("answer id not found in argument list"))?,
        10,
    )
    .map_err(|e| format!("{}", e))
}

pub fn decode_arg(args: &Value, name: &str) -> Result<String, String> {
    args[name]
        .as_str()
        .ok_or(format!("\"{}\" not found", name))
        .map(|x| x.to_string())
}

pub fn decode_bool_arg(args: &Value, name: &str) -> Result<bool, String> {
    args[name]
        .as_bool()
        .ok_or(format!("\"{}\" not found", name))
}

pub fn decode_string_arg(args: &Value, name: &str) -> Result<String, String> {
    let bytes = hex::decode(&decode_arg(args, name)?)
        .map_err(|e| format!("{}", e))?;
    std::str::from_utf8(&bytes)
        .map_err(|e| format!("{}", e))
        .map(|x| x.to_string())
}

pub fn decode_prompt(args: &Value) -> Result<String, String> {
    decode_string_arg(args, "prompt")
}

pub fn decode_num_arg<T>(args: &Value, name: &str) -> Result<T, String>
where
    T: NumCast,
{
    let num_str = decode_arg(args, name)?;
    decode_abi_number::<T>(&num_str)
        .map_err(|e| format!("failed to parse integer \"{}\": {}", num_str, e))
}

pub fn decode_int256(args: &Value, name: &str) -> Result<BigInt, String> {
    let num_str = decode_arg(args, name)?;
    decode_abi_bigint(&num_str)
        .map_err(|e| format!("failed to decode integer \"{}\": {}", num_str, e))
}

pub fn decode_array<F, T>(args: &Value, name: &str, validator: F) -> Result<Vec<T>, String> 
    where F: Fn(&Value) -> Option<T>
{
    let array = args[name]
        .as_array()
        .ok_or(format!("\"{}\" is invalid: must be array", name))?;
    let mut strings = vec![];
    for elem in array {
        strings.push(
            validator(&elem).ok_or(format!("invalid array element type"))?
        );
    }
    Ok(strings)
}