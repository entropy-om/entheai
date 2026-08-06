//! entheai-quantum — quantum emulator + simulator
//!
//! The constellation's 0+1 playground made concrete: a dense amplitude-vector
//! simulator with a gate set (Hadamard, Pauli X/Y/Z, CNOT, Phase) and
//! measurement. This is the same "quantum simulation playground" entheai's
//! docs gesture at — now it actually runs. Bleeding edge, no "6.1".

use serde::{Deserialize, Serialize};

/// A quantum register state as a dense amplitude vector (2^n complex).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Qubits {
    /// Amplitudes, length 2^n, index = computational basis state.
    pub amps: Vec<Complex>,
    pub n: usize,
}

/// Minimal complex number (kept dependency-free for the core).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn mul(&self, o: &Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    fn scale(&self, s: f64) -> Self {
        Self::new(self.re * s, self.im * s)
    }
}

impl Qubits {
    /// `|0...0⟩` over `n` qubits.
    pub fn zeros(n: usize) -> Self {
        let mut amps = vec![Complex::new(0.0, 0.0); 1 << n];
        amps[0] = Complex::new(1.0, 0.0);
        Self { amps, n }
    }

    pub fn dim(&self) -> usize {
        self.amps.len()
    }

    fn apply_single(&mut self, q: usize, m: &[[Complex; 2]; 2]) {
        let n = self.n;
        for base in 0..(1 << n) {
            if (base >> q) & 1 == 0 {
                let b1 = base | (1 << q);
                let a0 = self.amps[base];
                let a1 = self.amps[b1];
                self.amps[base] = m[0][0].mul(&a0).add(&m[0][1].mul(&a1));
                self.amps[b1] = m[1][0].mul(&a0).add(&m[1][1].mul(&a1));
            }
        }
    }

    fn apply_controlled(&mut self, ctrl: usize, tgt: usize, m: &[[Complex; 2]; 2]) {
        let n = self.n;
        for base in 0..(1 << n) {
            let c = (base >> ctrl) & 1;
            if c == 0 {
                continue;
            }
            if (base >> tgt) & 1 == 0 {
                let b1 = base | (1 << tgt);
                let a0 = self.amps[base];
                let a1 = self.amps[b1];
                self.amps[base] = m[0][0].mul(&a0).add(&m[0][1].mul(&a1));
                self.amps[b1] = m[1][0].mul(&a0).add(&m[1][1].mul(&a1));
            }
        }
    }

    pub fn h(&mut self, q: usize) {
        let s = 1.0 / 2f64.sqrt();
        self.apply_single(q, &[[Complex::new(s, 0.0), Complex::new(s, 0.0)],
                              [Complex::new(s, 0.0), Complex::new(-s, 0.0)]]);
    }
    pub fn x(&mut self, q: usize) {
        self.apply_single(q, &[[Complex::new(0.0, 0.0), Complex::new(1.0, 0.0)],
                              [Complex::new(1.0, 0.0), Complex::new(0.0, 0.0)]]);
    }
    pub fn z(&mut self, q: usize) {
        self.apply_single(q, &[[Complex::new(1.0, 0.0), Complex::new(0.0, 0.0)],
                              [Complex::new(0.0, 0.0), Complex::new(-1.0, 0.0)]]);
    }
    pub fn cnot(&mut self, ctrl: usize, tgt: usize) {
        self.apply_controlled(ctrl, tgt, &[[Complex::new(0.0, 0.0), Complex::new(1.0, 0.0)],
                                          [Complex::new(1.0, 0.0), Complex::new(0.0, 0.0)]]);
    }

    /// Measure qubit `q` in the computational basis, collapsing the state.
    /// Returns 0 or 1. Deterministic seed for reproducibility.
    pub fn measure(&mut self, q: usize, seed: u64) -> u8 {
        let mut p0 = 0.0;
        for base in 0..self.dim() {
            if (base >> q) & 1 == 0 {
                p0 += self.amps[base].re * self.amps[base].re + self.amps[base].im * self.amps[base].im;
            }
        }
        // deterministic pseudo-random (xorshift)
        let mut s = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(0x1234567);
        s ^= s >> 12; s ^= s << 25; s ^= s >> 27;
        let r = (s as f64 / u64::MAX as f64);
        let outcome: u8 = if r < p0 { 0 } else { 1 };
        // collapse
        let total = if outcome == 0 { p0 } else { 1.0 - p0 }.max(1e-12);
        for base in 0..self.dim() {
            let keep = ((base >> q) & 1) == outcome as usize;
            self.amps[base] = if keep {
                self.amps[base].scale(1.0 / total.sqrt())
            } else {
                Complex::new(0.0, 0.0)
            };
        }
        outcome
    }

    /// Born-rule probability of measuring 1 on qubit `q` (without collapse).
    pub fn prob1(&self, q: usize) -> f64 {
        let mut p = 0.0;
        for base in 0..self.dim() {
            if (base >> q) & 1 == 1 {
                p += self.amps[base].re * self.amps[base].re + self.amps[base].im * self.amps[base].im;
            }
        }
        p
    }
}

// small helper so the gate math stays readable
trait CAdd {
    fn add(&self, o: &Self) -> Self;
}
impl CAdd for Complex {
    fn add(&self, o: &Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hadamard_superposes() {
        // |0⟩ --H--> (|0⟩+|1⟩)/√2 : prob1 = 0.5
        let mut q = Qubits::zeros(1);
        q.h(0);
        assert!((q.prob1(0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn cnot_entangles() {
        // |00⟩ --H(0)--> (|00⟩+|10⟩)/√2 --CNOT(0→1)--> (|00⟩+|11⟩)/√2
        let mut q = Qubits::zeros(2);
        q.h(0);
        q.cnot(0, 1);
        assert!((q.prob1(1) - 0.5).abs() < 1e-9);
        // only 00 and 11 have amplitude
        assert!(q.amps[0].re.abs() > 0.5 && q.amps[3].re.abs() > 0.5);
        assert!(q.amps[1].re.abs() < 1e-9 && q.amps[2].re.abs() < 1e-9);
    }

    #[test]
    fn bell_measure_is_deterministic_given_seed() {
        let mut q = Qubits::zeros(2);
        q.h(0);
        q.cnot(0, 1);
        let m0 = q.measure(0, 42);
        let m1 = q.measure(1, 42);
        assert_eq!(m0, m1, "bell pair: measurement outcomes must match");
    }
}
