use curve25519_dalek::ristretto::RistrettoPoint;
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use generators::GeneratorsView;
use inner_product_proof;
use proof_transcript::ProofTranscript;
use std::clone::Clone;
use util;

use super::messages::*;

/// Dealer is an entry-point API for setting up a dealer
pub struct Dealer {}

impl Dealer {
    /// Creates a new dealer with the given parties and a number of bits
    pub fn new(
        n: usize,
        m: usize,
        transcript: &mut ProofTranscript,
    ) -> Result<DealerAwaitingValues, &'static str> {
        transcript.commit_u64(n as u64);
        transcript.commit_u64(m as u64);
        Ok(DealerAwaitingValues { n, m })
    }
}

/// When the dealer is initialized, it only knows the size of the set.
#[derive(Debug)]
pub struct DealerAwaitingValues {
    n: usize,
    m: usize,
}

impl DealerAwaitingValues {
    /// Combines commitments and computes challenge variables.
    pub fn receive_value_commitments(
        self,
        value_commitments: &Vec<ValueCommitment>,
        transcript: &mut ProofTranscript,
    ) -> Result<(DealerAwaitingPoly, ValueChallenge), (DealerAwaitingValues, &'static str)> {
        if self.m != value_commitments.len() {
            return Err((
                self,
                "Length of value commitments doesn't match expected length m",
            ));
        }

        let mut A = RistrettoPoint::identity();
        let mut S = RistrettoPoint::identity();

        for commitment in value_commitments.iter() {
            // Commit each V individually
            transcript.commit(commitment.V.compress().as_bytes());

            // Commit sums of As and Ss.
            A += commitment.A;
            S += commitment.S;
        }

        transcript.commit(A.compress().as_bytes());
        transcript.commit(S.compress().as_bytes());

        let y = transcript.challenge_scalar();
        let z = transcript.challenge_scalar();
        let value_challenge = ValueChallenge { y, z };

        Ok((
            DealerAwaitingPoly {
                n: self.n,
                m: self.m,
                value_challenge: value_challenge.clone(),
            },
            value_challenge,
        ))
    }
}

#[derive(Debug)]
pub struct DealerAwaitingPoly {
    n: usize,
    m: usize,
    value_challenge: ValueChallenge,
}

impl DealerAwaitingPoly {
    pub fn receive_poly_commitments(
        self,
        poly_commitments: &Vec<PolyCommitment>,
        transcript: &mut ProofTranscript,
    ) -> Result<(DealerAwaitingShares, PolyChallenge), (DealerAwaitingPoly, &'static str)> {
        if self.m != poly_commitments.len() {
            return Err((
                self,
                "Length of poly commitments doesn't match expected length m",
            ));
        }

        // Commit sums of T1s and T2s.
        let mut T1 = RistrettoPoint::identity();
        let mut T2 = RistrettoPoint::identity();
        for commitment in poly_commitments.iter() {
            T1 += commitment.T_1;
            T2 += commitment.T_2;
        }
        transcript.commit(T1.compress().as_bytes());
        transcript.commit(T2.compress().as_bytes());

        let x = transcript.challenge_scalar();
        let poly_challenge = PolyChallenge { x };

        Ok((
            DealerAwaitingShares {
                n: self.n,
                m: self.m,
                value_challenge: self.value_challenge,
                poly_challenge: poly_challenge.clone(),
            },
            poly_challenge,
        ))
    }
}

#[derive(Debug)]
pub struct DealerAwaitingShares {
    n: usize,
    m: usize,
    value_challenge: ValueChallenge,
    poly_challenge: PolyChallenge,
}

impl DealerAwaitingShares {
    pub fn receive_shares(
        self,
        proof_shares: &Vec<ProofShare>,
        gen: &GeneratorsView,
        transcript: &mut ProofTranscript,
    ) -> Result<Proof, (DealerAwaitingShares, &'static str)> {
        if self.m != proof_shares.len() {
            return Err((
                self,
                "Length of proof shares doesn't match expected length m",
            ));
        }

        for (_j, proof_share) in proof_shares.iter().enumerate() {
            if proof_share
                .verify_share(&self.value_challenge, &self.poly_challenge)
                .is_err()
            {
                return Err((
                    self,
                    "One of the proof shares is invalid", // TODO: print which one (j) is invalid
                ));
            }
        }

        let value_commitments = proof_shares
            .iter()
            .map(|ps| ps.value_commitment.V.clone())
            .collect();
        let A = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |A, ps| {
                A + ps.value_commitment.A
            });
        let S = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |S, ps| {
                S + ps.value_commitment.S
            });
        let T_1 = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |T_1, ps| {
                T_1 + ps.poly_commitment.T_1
            });
        let T_2 = proof_shares
            .iter()
            .fold(RistrettoPoint::identity(), |T_2, ps| {
                T_2 + ps.poly_commitment.T_2
            });
        let t = proof_shares
            .iter()
            .fold(Scalar::zero(), |acc, ps| acc + ps.t_x);
        let t_x_blinding = proof_shares
            .iter()
            .fold(Scalar::zero(), |acc, ps| acc + ps.t_x_blinding);
        let e_blinding = proof_shares
            .iter()
            .fold(Scalar::zero(), |acc, ps| acc + ps.e_blinding);
        transcript.commit(t.as_bytes());
        transcript.commit(t_x_blinding.as_bytes());
        transcript.commit(e_blinding.as_bytes());

        // Get a challenge value to combine statements for the IPP
        let w = transcript.challenge_scalar();
        let Q = w * gen.pedersen_generators.B;

        let l_vec: Vec<Scalar> = proof_shares
            .iter()
            .flat_map(|ps| ps.l_vec.clone().into_iter())
            .collect();
        let r_vec: Vec<Scalar> = proof_shares
            .iter()
            .flat_map(|ps| ps.r_vec.clone().into_iter())
            .collect();
        let ipp_proof = inner_product_proof::InnerProductProof::create(
            transcript,
            &Q,
            util::exp_iter(self.value_challenge.y.invert()),
            gen.G.to_vec(),
            gen.H.to_vec(),
            l_vec.clone(),
            r_vec.clone(),
        );

        Ok(Proof {
            n: self.n,
            value_commitments,
            A,
            S,
            T_1,
            T_2,
            t_x: t,
            t_x_blinding,
            e_blinding,
            ipp_proof,
        })
    }
}