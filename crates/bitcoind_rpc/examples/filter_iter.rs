#![allow(clippy::print_stdout, clippy::print_stderr)]
use std::time::Instant;

use anyhow::Context;
use bdk_bitcoind_rpc::bip158::{Event, FilterIter};
use bdk_chain::bitcoin::{constants::genesis_block, secp256k1::Secp256k1, Network};
use bdk_chain::indexer::keychain_txout::KeychainTxOutIndex;
use bdk_chain::local_chain::LocalChain;
use bdk_chain::miniscript::Descriptor;
use bdk_chain::{BlockId, ConfirmationBlockTime, IndexedTxGraph, SpkIterator};
use bdk_testenv::anyhow;

// This example shows how BDK chain and tx-graph structures are updated using compact
// filters syncing. Assumes a connection can be made to a bitcoin node via a ContextVM
// MCP-over-Nostr server, configured with environment variables `SERVER_PUBKEY` and
// `RELAY_URL` (the latter defaults to `ws://localhost:10547`).

// Usage: `cargo run -p bdk_bitcoind_rpc --example filter_iter`

const EXTERNAL: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/0/*)";
const INTERNAL: &str = "tr([83737d5e/86'/1'/0']tpubDDR5GgtoxS8fJyjjvdahN4VzV5DV6jtbcyvVXhEKq2XtpxjxBXmxH3r8QrNbQqHg4bJM1EGkxi7Pjfkgnui9jQWqS7kxHvX6rhUeriLDKxz/1/*)";
const SPK_COUNT: u32 = 25;
const NETWORK: Network = Network::Regtest;

const START_HEIGHT: u32 = 20;
const START_HASH: &str = "76ca30d3f54fd45bb37822c3ade3f6a602631457f69615619dd87421e4a2bec1";

fn main() -> anyhow::Result<()> {
    // Setup receiving chain and graph structures.

    let secp = Secp256k1::new();
    let (descriptor, _) = Descriptor::parse_descriptor(&secp, EXTERNAL)?;
    let (change_descriptor, _) = Descriptor::parse_descriptor(&secp, INTERNAL)?;
    let (mut chain, _) = LocalChain::from_genesis_hash(genesis_block(NETWORK).block_hash());

    let mut graph = IndexedTxGraph::<ConfirmationBlockTime, KeychainTxOutIndex<&str>>::new({
        let mut index = KeychainTxOutIndex::default();
        index.insert_descriptor("external", descriptor.clone())?;
        index.insert_descriptor("internal", change_descriptor.clone())?;
        index
    });

    // Assume a minimum birthday height
    let block = BlockId {
        height: START_HEIGHT,
        hash: START_HASH.parse()?,
    };
    let _ = chain.insert_block(block)?;

    // Configure RPC client
    let server_pubkey = "7807ffe23010b8961a1e1aecb1cbf82b58e7cf99401cbb7874b73e21b1b12629".to_string();
    let relay_url =
        std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:10547".to_string());
    let rpc_client = bitcoincore_rpc::Client::new(vec![relay_url], server_pubkey)?;

    // Diagnostic: dump the server's advertised tools and their input schemas.
    rpc_client.dump_tool_schemas()?;


    // Initialize `FilterIter`
    let mut spks = vec![];
    for (_, desc) in graph.index.keychains() {
        spks.extend(SpkIterator::new_with_range(desc, 0..SPK_COUNT).map(|(_, s)| s));
    }
    let iter = FilterIter::new(&rpc_client, chain.tip(), spks);

    let start = Instant::now();

    for res in iter {
        let Event { cp, block } = res?;
        let height = cp.height();
        let _ = chain.apply_update(cp)?;
        if let Some(block) = block {
            let _ = graph.apply_block_relevant(&block, height);
            println!("Matched block {height}");
        }
    }

    println!("\ntook: {}s", start.elapsed().as_secs());
    println!("Local tip: {}", chain.tip().height());
    let unspent: Vec<_> = graph
        .graph()
        .filter_chain_unspents(
            &chain,
            chain.tip().block_id(),
            Default::default(),
            graph.index.outpoints().clone(),
        )
        .collect();
    if !unspent.is_empty() {
        println!("\nUnspent");
        for (index, utxo) in unspent {
            // (k, index) | value | outpoint |
            println!("{:?} | {} | {}", index, utxo.txout.value, utxo.outpoint);
        }
    }

    for canon_tx in graph.graph().list_canonical_txs(
        &chain,
        chain.tip().block_id(),
        bdk_chain::CanonicalizationParams::default(),
    ) {
        if !canon_tx.chain_position.is_confirmed() {
            eprintln!(
                "ERROR: canonical tx should be confirmed {}",
                canon_tx.tx_node.txid
            );
        }
    }

    Ok(())
}
