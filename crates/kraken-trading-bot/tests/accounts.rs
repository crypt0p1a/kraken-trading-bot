//! Several Kraken accounts in one database: the same managers, nothing shared

use kraken_spot_api::trading::{OrderSide, OrderStatus};
use kraken_trading_bot::bot::Bot;
use kraken_trading_bot::config::{BotConfig, ManagerConfig, ManagerMode};
use kraken_trading_bot::exchange::SimulatedExchange;
use kraken_trading_bot::ledger;
use kraken_trading_bot::order::BotOrder;
use kraken_trading_bot::storage::{CommandState, FORCED_TICK, ManagerState, Storage};
use rust_decimal::Decimal;

/// Two accounts in one database run the same managers, and never see each other's
#[test]
fn keeps_the_accounts_apart_in_one_database() {
    let directory = std::env::temp_dir().join(format!(
        "kraken-bot-accounts-{}-{}",
        std::process::id(),
        kraken_trading_bot::clock::now_millis()
    ));
    let path = directory.join("bot.sqlite");
    let main = Storage::open(&path).unwrap();
    let alice = Storage::open_account(&path, "alice").unwrap();
    // opened again: the same account, not a new one
    let alice_again = Storage::open_account(&path, "alice").unwrap();

    main.set_state("watermark", "1").unwrap();
    alice.set_state("watermark", "2").unwrap();
    assert_eq!(main.state("watermark").unwrap().as_deref(), Some("1"));
    assert_eq!(
        alice_again.state("watermark").unwrap().as_deref(),
        Some("2")
    );

    // the same manager id on both
    let state = |budget: &str| ManagerState {
        config: ManagerConfig {
            id: 7,
            pair: "XBTEUR".into(),
            budget: budget.parse().unwrap(),
            buy_below: "1".parse().unwrap(),
            sell_above: "2".parse().unwrap(),
            mode: ManagerMode::Active,
            holding: None,
        },
        balances: ledger::compute(budget.parse().unwrap(), &[]),
        last_price: "70000".parse().unwrap(),
        at: 1_000,
    };
    main.save_manager(&state("30")).unwrap();
    alice.save_manager(&state("50")).unwrap();
    assert_eq!(
        main.manager_balances(7).unwrap().unwrap().0.quote_available,
        "30".parse().unwrap()
    );
    assert_eq!(
        alice
            .manager_balances(7)
            .unwrap()
            .unwrap()
            .0
            .quote_available,
        "50".parse().unwrap()
    );
    assert_eq!(main.snapshot_count(7).unwrap(), 1);
    assert_eq!(alice.snapshot_count(7).unwrap(), 1);

    // a tick asked for one account isn't asked for the other
    main.request_command(FORCED_TICK, 1).unwrap();
    assert_eq!(
        main.command(FORCED_TICK).unwrap(),
        Some(CommandState::Refresh)
    );
    assert_eq!(
        alice.command(FORCED_TICK).unwrap(),
        Some(CommandState::Idle)
    );
    assert!(!alice.start_command(FORCED_TICK, 2).unwrap());
    assert!(main.start_command(FORCED_TICK, 2).unwrap());

    // the orders, with the snapshots of the whole account
    let order = |txid: &str| BotOrder {
        txid: txid.into(),
        manager_id: 7,
        pair: "XBTEUR".into(),
        side: OrderSide::Buy,
        status: OrderStatus::Open,
        volume: "0.001".parse().unwrap(),
        volume_executed: Decimal::ZERO,
        cost: Decimal::ZERO,
        fee: Decimal::ZERO,
        limit_price: None,
        average_price: None,
        open_time: 1.0,
        close_time: None,
        expire_time: None,
    };
    main.upsert_orders(&[order("M")]).unwrap();
    alice.upsert_orders(&[order("A")]).unwrap();
    let txids = |storage: &Storage| -> Vec<String> {
        storage
            .open_orders()
            .unwrap()
            .into_iter()
            .map(|it| it.txid)
            .collect()
    };
    assert_eq!(txids(&main), ["M"]);
    assert_eq!(txids(&alice), ["A"]);
    assert_eq!(alice.order("M").unwrap(), None);
    drop((main, alice, alice_again));
    std::fs::remove_dir_all(directory).unwrap();
}

/// Two bots with the same configuration, each on its account: each spends its own money
#[tokio::test]
async fn trades_the_same_managers_on_each_account() {
    let directory = std::env::temp_dir().join(format!(
        "kraken-bot-two-bots-{}-{}",
        std::process::id(),
        kraken_trading_bot::clock::now_millis()
    ));
    let path = directory.join("bot.sqlite");
    let config = BotConfig::parse(
        "dry_run: false
managers:
  - {id: 1, pair: XBTUSD, budget: 30, buy_below: 1, sell_above: 1.5}",
    )
    .unwrap();
    let exchange = |money: &str| {
        let exchange = SimulatedExchange::new(1_800_000_000.0);
        exchange.add_pair(
            "XXBTZUSD",
            "XBTUSD",
            "XXBT",
            "ZUSD",
            "50000".parse().unwrap(),
        );
        exchange.set_fee_percent("0.25".parse().unwrap());
        exchange.deposit("ZUSD", money.parse().unwrap());
        exchange
    };
    let mut main = Bot::new(
        exchange("100"),
        Storage::open(&path).unwrap(),
        config.clone(),
    );
    let mut alice = Bot::new(
        exchange("500"),
        Storage::open_account(&path, "alice").unwrap(),
        config,
    );
    main.prepare().await.unwrap();
    alice.prepare().await.unwrap();

    let (first, second) = tokio::join!(main.tick(), alice.tick());
    first.unwrap();
    second.unwrap();

    for bot in [&main, &alice] {
        let orders = bot.storage().orders_of(1).unwrap();
        assert_eq!(orders.len(), 1, "one buy for each account");
        assert_eq!(bot.storage().open_orders().unwrap(), orders);
    }
    assert_eq!(
        main.storage().last_accounts().unwrap()[0].cash,
        "100".parse::<Decimal>().unwrap()
    );
    assert_eq!(
        alice.storage().last_accounts().unwrap()[0].cash,
        "500".parse::<Decimal>().unwrap()
    );
    drop((main, alice));
    std::fs::remove_dir_all(directory).unwrap();
}
