// CITA
// Copyright 2016-2018 Cryptape Technologies LLC.

// This program is free software: you can redistribute it
// and/or modify it under the terms of the GNU General Public
// License as published by the Free Software Foundation,
// either version 3 of the License, or (at your option) any
// later version.

// This program is distributed in the hope that it will be
// useful, but WITHOUT ANY WARRANTY; without even the implied
// warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR
// PURPOSE. See the GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

#![feature(try_from)]

extern crate cita_crypto as crypto;
extern crate cita_types as types;
extern crate grpc;
#[macro_use]
extern crate logger;
extern crate protobuf;
extern crate rlp;
extern crate rustc_serialize;
#[macro_use]
extern crate serde_derive;
extern crate tls_api;
extern crate util;

pub mod protos;
pub use protos::*;
mod autoimpl;
pub mod router;

use crypto::{CreateKey, KeyPair, Message as SignMessage, PrivKey, PubKey, Sign, Signature, SIGNATURE_BYTES_LEN};
use protobuf::RepeatedField;
use rlp::{Decodable, DecoderError, Encodable, RlpStream, UntrustedRlp};
use rustc_serialize::hex::ToHex;
use std::convert::{From, TryFrom, TryInto};
use std::ops::Deref;
use std::result::Result::Err;
use types::H256;
use util::{merklehash, Hashable};

pub use autoimpl::{Message, MsgClass, OperateType, Origin, RawBytes, TryFromConvertError, TryIntoConvertError,
                   ZERO_ORIGIN};

//TODO respone contain error
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct TxResponse {
    pub hash: H256,
    pub status: String,
}

impl TxResponse {
    pub fn new(hash: H256, status: String) -> Self {
        TxResponse {
            hash: hash,
            status: status,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
pub struct State(pub Vec<Vec<u8>>);

impl From<RichStatus> for Status {
    fn from(rich_status: RichStatus) -> Self {
        let mut status = Status::new();
        status.hash = rich_status.get_hash().to_vec();
        status.height = rich_status.get_height();
        status
    }
}

impl Transaction {
    /// Signs the transaction by PrivKey.
    pub fn sign(&self, sk: PrivKey) -> SignedTransaction {
        let keypair = KeyPair::from_privkey(sk).unwrap();
        let pubkey = keypair.pubkey();
        let unverified_tx = self.build_unverified(sk);

        // Build SignedTransaction
        let mut signed_tx = SignedTransaction::new();
        signed_tx.set_signer(pubkey.to_vec());
        let bytes: Vec<u8> = (&unverified_tx).try_into().unwrap();
        signed_tx.set_tx_hash(bytes.crypt_hash().to_vec());
        signed_tx.set_transaction_with_sig(unverified_tx);
        signed_tx
    }

    /// Build UnverifiedTransaction
    pub fn build_unverified(&self, sk: PrivKey) -> UnverifiedTransaction {
        let mut unverified_tx = UnverifiedTransaction::new();
        let bytes: Vec<u8> = self.try_into().unwrap();
        let hash = bytes.crypt_hash();
        unverified_tx.set_transaction(self.clone());
        let signature = Signature::sign(&sk, &SignMessage::from(hash)).unwrap();
        unverified_tx.set_signature(signature.to_vec());
        unverified_tx.set_crypto(Crypto::SECP);
        unverified_tx
    }
}

impl UnverifiedTransaction {
    /// Try to recover the public key.
    pub fn recover_public(&self) -> Result<(PubKey, H256), (H256, String)> {
        let bytes: Vec<u8> = self.get_transaction().try_into().unwrap();
        let hash = bytes.crypt_hash();
        let tx_hash = self.crypt_hash();
        if self.get_signature().len() != SIGNATURE_BYTES_LEN {
            trace!("Invalid signature length {}", hash);
            Err((tx_hash, String::from("Invalid signature length")))
        } else {
            match self.get_crypto() {
                Crypto::SECP => {
                    let signature = Signature::from(self.get_signature());
                    match signature.recover(&hash) {
                        Ok(pubkey) => Ok((pubkey, tx_hash)),
                        _ => {
                            trace!("Recover error {}", tx_hash);
                            Err((tx_hash, String::from("Recover error")))
                        }
                    }
                }
                _ => {
                    trace!("Unexpected crypto {}", tx_hash);
                    Err((tx_hash, String::from("Unexpected crypto")))
                }
            }
        }
    }

    pub fn crypt_hash(&self) -> H256 {
        let bytes: Vec<u8> = self.try_into().unwrap();
        bytes.crypt_hash()
    }

    pub fn tx_verify_req_msg(&self) -> VerifyTxReq {
        let bytes: Vec<u8> = self.get_transaction().try_into().unwrap();
        let hash = bytes.crypt_hash();
        let mut verify_tx_req = VerifyTxReq::new();
        verify_tx_req.set_valid_until_block(self.get_transaction().get_valid_until_block());
        // tx hash
        verify_tx_req.set_hash(hash.to_vec());
        verify_tx_req.set_crypto(self.get_crypto());
        verify_tx_req.set_signature(self.get_signature().to_vec());
        verify_tx_req.set_nonce(self.get_transaction().get_nonce().to_string());
        verify_tx_req.set_value(self.get_transaction().get_value().to_vec());
        verify_tx_req.set_chain_id(self.get_transaction().get_chain_id());
        verify_tx_req.set_quota(self.get_transaction().get_quota());

        // unverified tx hash
        let tx_hash = self.crypt_hash();
        verify_tx_req.set_tx_hash(tx_hash.to_vec());
        verify_tx_req
    }
}

impl Deref for SignedTransaction {
    type Target = UnverifiedTransaction;

    fn deref(&self) -> &Self::Target {
        &self.get_transaction_with_sig()
    }
}

impl SignedTransaction {
    /// Try to verify transaction and recover sender.
    pub fn verify_transaction(transaction: UnverifiedTransaction) -> Result<Self, H256> {
        let (public, tx_hash) = transaction.recover_public().map_err(|(hash, _)| hash)?;
        let mut signed_tx = SignedTransaction::new();
        signed_tx.set_signer(public.to_vec());
        signed_tx.set_tx_hash(tx_hash.to_vec());
        signed_tx.set_transaction_with_sig(transaction);
        Ok(signed_tx)
    }

    pub fn crypt_hash(&self) -> H256 {
        H256::from(self.tx_hash.as_slice())
    }
}

impl Eq for Proof {}

impl Decodable for Proof {
    fn decode(rlp: &UntrustedRlp) -> Result<Self, DecoderError> {
        rlp.decoder()
            .decode_value(|bytes| Ok(Proof::try_from(bytes).unwrap()))
    }
}

impl Encodable for Proof {
    fn rlp_append(&self, s: &mut RlpStream) {
        let b: Vec<u8> = self.try_into().unwrap();
        s.encoder().encode_value(&b);
    }
}

impl Block {
    pub fn crypt_hash(&self) -> H256 {
        self.get_header().crypt_hash()
    }

    pub fn crypt_hash_hex(&self) -> String {
        self.get_header().crypt_hash_hex()
    }

    pub fn check_hash(&self) -> bool {
        self.get_body().transactions_root().0 == *self.get_header().get_transactions_root()
    }

    pub fn block_verify_req(&self, request_id: u64) -> VerifyBlockReq {
        let mut reqs: Vec<VerifyTxReq> = Vec::new();
        let signed_txs = self.get_body().get_transactions();
        for signed_tx in signed_txs {
            let signer = signed_tx.get_signer();
            let unverified_tx = signed_tx.get_transaction_with_sig();
            let mut verify_tx_req = unverified_tx.tx_verify_req_msg();
            verify_tx_req.set_signer(signer.to_vec());
            reqs.push(verify_tx_req);
        }
        let mut verify_blk_req = VerifyBlockReq::new();
        verify_blk_req.set_id(request_id);
        verify_blk_req.set_reqs(RepeatedField::from_vec(reqs));
        verify_blk_req
    }
}

impl BlockHeader {
    pub fn crypt_hash(&self) -> H256 {
        let bytes: Vec<u8> = self.try_into().unwrap();
        bytes.crypt_hash()
    }

    pub fn crypt_hash_hex(&self) -> String {
        let bytes: Vec<u8> = self.try_into().unwrap();
        bytes.crypt_hash().to_hex()
    }
}

impl BlockBody {
    pub fn transaction_hashes(&self) -> Vec<H256> {
        self.get_transactions()
            .iter()
            .map(|ts| H256::from_slice(ts.get_tx_hash()))
            .collect()
    }

    pub fn transactions_root(&self) -> H256 {
        merklehash::MerkleTree::from_hashes(self.transaction_hashes().clone()).get_root_hash()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn create_tx() {
        use super::{CreateKey, KeyPair, Transaction};
        let keypair = KeyPair::gen_keypair();
        let pv = keypair.privkey();

        let data = vec![1];
        let mut tx = Transaction::new();
        tx.set_data(data);
        tx.set_nonce("0".to_string());
        tx.set_to("123".to_string());
        tx.set_valid_until_block(99999);
        tx.set_quota(999999999);

        let signed_tx = tx.sign(*pv);
        assert_eq!(
            signed_tx.crypt_hash(),
            signed_tx.get_transaction_with_sig().crypt_hash()
        );
    }

}
