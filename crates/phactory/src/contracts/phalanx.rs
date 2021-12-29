use crate::contracts;
use crate::contracts::{AccountId, NativeContext};
use crate::secret_channel::Payload;
use crate::side_task::async_side_task::AsyncSideTask;
use parity_scale_codec::{Decode, Encode};
use phala_mq::{contract_id256, MessageOrigin};
use phala_types::contract::command_topic;
use serde_json;
use std::cmp;
use std::collections::HashMap;
use super::{TransactionError, TransactionResult};
use surf;

extern crate runtime as chain;



///////////////////////////////////////////////////////////////////////////////
//PHALANX LOGIC
///////////////////////////////////////////////////////////////////////////////
#[derive(PartialEq, Eq, Debug, Clone, Encode, Decode)]
pub enum Status {
  Active,
  Inactive
}

#[derive(PartialEq, Eq, Debug, Clone, Encode, Decode)]
pub enum Side {
  Buy,
  Sell
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Order {
  pub acct: AccountId,
  pub status: Status,
  pub side: Side,
  pub size: u64,
  pub filled: u64
}

impl Order {
  pub fn new(acct:AccountId, side: Side, size: u64) -> Self {
    Order {
      acct,
      status: Status::Active,
      side,
      size,
      filled: 0
    }
  }

  pub fn fill(&mut self, size: u64){
    self.filled = self.filled + size;
    if self.filled == self.size {
      self.cancel();
    }
  }

  pub fn remaining(&self) -> u64 {
    self.size - self.filled
  }

  pub fn cancel(&mut self){
    self.status = Status::Inactive;
  }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Trade {
  pub side: Side,
  pub size: u64,
  pub px_u64: u64
}

impl Trade {
  pub fn new(side:Side, size: u64, px_u64: u64) -> Self {
    Trade {
      side,
      size,
      px_u64
    }
  }
}

#[derive(Debug)]
pub struct Phalanx {
  pub bids: Vec<Order>,
  pub asks: Vec<Order>,
  trade_histories: HashMap<AccountId, Vec::<Trade>>
}

impl Phalanx {
  pub fn new() -> Self {
    Phalanx {
      bids: Vec::new(),
      asks: Vec::new(),
      trade_histories: HashMap::new()
    }
  }

  pub fn insert_order(&mut self, order: Order, px_u64: u64){
    if order.status == Status::Active {
      //Users are only allowed to have one active order at a time.
      //If the user has any existing orders, we cancel them first.
      self.cancel_user_orders(order.acct.clone());
      if order.side == Side::Buy {
        self.match_buy_order(order, px_u64);
      }else if order.side == Side::Sell {
        self.match_sell_order(order, px_u64);
      }
    }
    //Remove inactive orders
    self.cleanup();
  }

  pub fn find_order_by_acct(&mut self, acct: AccountId) -> Option<&mut Order>{
    let bids = &mut self.bids;
    for bid in bids.iter_mut() {
      if bid.acct == acct {
        return Some(bid);
      }
    }
    let asks = &mut self.asks;
    for ask in asks.iter_mut() {
      if ask.acct == acct {
        return Some(ask);
      }
    }
    None
  }

  pub fn cancel_user_orders(&mut self, acct: AccountId){
    match self.find_order_by_acct(acct) {
      Some(order) => order.cancel(),
      None => {}
    }
  }

  fn match_buy_order(&mut self, mut buy_order: Order, px_u64: u64){
    let asks = &mut self.asks;
    for ask in asks.iter_mut() {
      if Self::pretrade_checks(ask, px_u64) {
        Self::exec_trade(&mut buy_order, ask, px_u64,  &mut self.trade_histories);
      }
      if buy_order.remaining() == 0 {
        break;
      }
    }
    self.bids.push(buy_order);
  }

  fn match_sell_order(&mut self, mut sell_order: Order, px_u64: u64){
    let bids = &mut self.bids;
    for bid in bids.iter_mut() {
      if Self::pretrade_checks(bid, px_u64) {
        Self::exec_trade(bid, &mut sell_order, px_u64, &mut self.trade_histories);
      }
      if sell_order.remaining() == 0 {
        break;
      }
    }
    self.asks.push(sell_order);
  }

  fn pretrade_checks(order: &Order, px_u64: u64) -> bool {
    let mut is_valid = true;
    if order.status != Status::Active {
      is_valid = false;
    }
    is_valid
  }

  // This function needs to:
  // 1) Store the size filled by both the buy order and the sell order
  // 2) Save the trade to the trade history vec of both the buyer and seller
  // 3) Transfer the corresponding tokens to/from the buyer and seller
  fn exec_trade(
    buy_order: &mut Order,
    sell_order: &mut Order,
    px_u64: u64,
    trade_histories: &mut HashMap<AccountId, Vec<Trade>>
  ) {
    let exec_size = cmp::min(buy_order.remaining(), sell_order.remaining());
    buy_order.fill(exec_size);
    sell_order.fill(exec_size);

    // Save the trade to both buyer/seller account's trade histories
    let buy_trade = Trade::new(Side::Buy, exec_size, px_u64);
    let buyer_trade_history = trade_histories.get(&buy_order.acct);
    match buyer_trade_history {
      Some(buyer_trade_history_prev) => {
        let mut buyer_trade_history = buyer_trade_history_prev.to_vec();
        buyer_trade_history.push(buy_trade);
        trade_histories.insert(buy_order.acct.clone(), buyer_trade_history);
      },
      None => {
        let buyer_trade_history = vec!(buy_trade);
        trade_histories.insert(buy_order.acct.clone(), buyer_trade_history);
      }
    };

    let sell_trade = Trade::new(Side::Sell, exec_size, px_u64);
    let seller_trade_history = trade_histories.get(&sell_order.acct);
    match seller_trade_history {
      Some(seller_trade_history_prev) => {
        let mut seller_trade_history = seller_trade_history_prev.to_vec();
        seller_trade_history.push(sell_trade);
        trade_histories.insert(sell_order.acct.clone(), seller_trade_history);
      },
      None => {
        let seller_trade_history = vec!(sell_trade);
        trade_histories.insert(sell_order.acct.clone(), seller_trade_history);
      }
    };

    println!("TRADE EXECUTED @ {:?}, SIZE={}", px_u64, exec_size);
    //TODO: finish the rest of the logic when executing a trade

  }

  fn cleanup(&mut self){
    self.bids.retain(|order| order.status == Status::Active);
    self.asks.retain(|order| order.status == Status::Active);
  }
}


///////////////////////////////////////////////////////////////////////////////
// PHALA CONTRACT IMPLEMENTATION
///////////////////////////////////////////////////////////////////////////////
use phala_types::messaging::PhalanxCommand;
type Command = PhalanxCommand;

#[derive(Encode, Decode, Debug)]
pub enum Request {
  GetOrder,
  GetTradeHistory
}

#[derive(Encode, Decode, Debug, Clone)]
pub enum Response {
  ActiveOrder(Order),
  TradeHistory(Vec<Trade>),
  None
}

#[derive(Encode, Decode, Debug)]
pub enum Error {
  GenericError
}

impl contracts::NativeContract for Phalanx {
  type Cmd = Command;
  type QReq = Request;
  type QResp = Result<Response, Error>;

  fn id(&self) -> contracts::ContractId32 {
    contracts::PHALANX
  }

  fn handle_command(
    &mut self,
    context: &mut NativeContext,
    origin: MessageOrigin,
    cmd: Command
  ) -> TransactionResult {

    match cmd {
      Command::SendOrder { is_buy, size } => {

        println!("Command received: {:?}", &cmd);

        //Get sender account
        let sender = match &origin {
          MessageOrigin::AccountId(acct) => AccountId::from(*acct.as_fixed_bytes()),
          _ => return Err(TransactionError::BadOrigin)
        };

        let block_number = context.block.block_number;
        let duration = 2;
        let mq = context.mq().clone();
        let id = self.id();

        let task = AsyncSideTask::spawn(
          block_number,
          duration,
          async move {
            println!("fetching price from HTTP request...");                                              
            let mut resp = surf::get("https://min-api.cryptocompare.com/data/price?fsym=DOT&tsyms=USD").send().await.unwrap();
            let result = resp.body_string().await.unwrap();
            result
          },
          move |result, _context| {
            let px_json: serde_json::Value = serde_json::from_str(result.unwrap().as_str()).expect("broken price");
            let px = px_json.get("USD").unwrap().as_f64().unwrap_or(f64::NAN);
            let px_u64 = (px * 100.0) as u64;

            let acct_bytes: [u8; 32] = *sender.as_ref();

            let command = Command::SendOrderWithPx { acct_bytes, is_buy, size, px_u64 };
            let message = Payload::Plain(command);
            mq.sendto(&message, command_topic(contract_id256(id)));
          }
        );
        context.block.side_task_man.add_task(task);
        Ok(())      
      },
      Command::SendOrderWithPx { acct_bytes, is_buy, size, px_u64 } => {

        // Only the contract itself is allowed to make this command
        if origin != MessageOrigin::native_contract(self.id()) {
          return Err(TransactionError::BadOrigin);
        }

        println!("Current market price (2 decimal places): {:?}", px_u64);
        let acct = AccountId::from(acct_bytes);

        if is_buy {
          let o = Order::new(acct, Side::Buy, size);
          self.insert_order(o, px_u64);
        }else {
          let o = Order::new(acct, Side::Sell, size);
          self.insert_order(o, px_u64);
        }

        println!("*******************************************************************************");
        println!("{:?}", self);
        println!("*******************************************************************************");
        Ok(())
      },
      Command::CancelOrder => {

        println!("Command received: {:?}", &cmd);

        //Get sender account
        let sender = match &origin {
          MessageOrigin::AccountId(acct) => AccountId::from(*acct.as_fixed_bytes()),
          _ => return Err(TransactionError::BadOrigin)
        };

        self.cancel_user_orders(sender);
        println!("*******************************************************************************");
        println!("{:?}", self);
        println!("*******************************************************************************");
        Ok(())
      }
    }
  }

  fn handle_query(
    &mut self,
    origin: Option<&chain::AccountId>,
    req: Request
  ) -> Result<Response, Error> {

    match req {
      Request::GetOrder => {
        if let Some(acct) = origin {
          let o = self.find_order_by_acct(acct.clone());
          if let Some(order) = o {
            if order.status == Status::Active{
              return Ok(Response::ActiveOrder(order.clone()));
            }
          }
        }
        Ok(Response::None)
      },
      Request::GetTradeHistory => {
        if let Some(acct) = origin {
          if let Some(trade_history) = self.trade_histories.get(acct) {
            return Ok(Response::TradeHistory(trade_history.to_vec()));
          }
        }
        Ok(Response::None)
      }
    }
  }
}  