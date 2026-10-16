// The responder side of Noise_IK_25519_ChaChaPoly_BLAKE2s, written from the Noise
// Protocol Framework specification (revision 34, https://noiseprotocol.org/noise.html).
// It must interoperate byte for byte with the Rust client (snow); `npm test` and the
// Rust `selftest --url` check that.
//
// IK:   <- s
//       ...
//       -> e, es, s, ss
//       <- e, ee, se

import { x25519 } from "@noble/curves/ed25519.js";
import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { blake2s } from "@noble/hashes/blake2.js";
import { hmac } from "@noble/hashes/hmac.js";

const PROTOCOL = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
export const PROLOGUE = new TextEncoder().encode("bot2bot/1");
const DHLEN = 32;
const HASHLEN = 32;
const TAG = 16;

const concat = (...parts: Uint8Array[]): Uint8Array => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};

const hash = (data: Uint8Array) => blake2s(data);
const mac = (key: Uint8Array, data: Uint8Array) => hmac(blake2s, key, data);

/** HKDF from the spec (section 4.3), two or three outputs. */
function hkdf(ck: Uint8Array, ikm: Uint8Array, n: 2 | 3): Uint8Array[] {
  const temp = mac(ck, ikm);
  const o1 = mac(temp, Uint8Array.of(1));
  const o2 = mac(temp, concat(o1, Uint8Array.of(2)));
  if (n === 2) return [o1, o2];
  return [o1, o2, mac(temp, concat(o2, Uint8Array.of(3)))];
}

/** ChaChaPoly nonce: 4 zero bytes then the 64-bit counter, little-endian. */
function nonce(n: bigint): Uint8Array {
  const out = new Uint8Array(12);
  new DataView(out.buffer).setBigUint64(4, n, true);
  return out;
}

export class CipherState {
  /** Key and nonce counter; public so a session can be saved and restored. */
  k: Uint8Array | null;
  n: bigint;
  constructor(k: Uint8Array | null = null, n = 0n) {
    this.k = k;
    this.n = n;
  }

  hasKey(): boolean {
    return this.k !== null;
  }

  encrypt(ad: Uint8Array, plain: Uint8Array): Uint8Array {
    if (!this.k) return plain;
    const out = chacha20poly1305(this.k, nonce(this.n), ad).encrypt(plain);
    this.n += 1n;
    return out;
  }

  /** Throws when the message fails authentication; the nonce only advances on success. */
  decrypt(ad: Uint8Array, data: Uint8Array): Uint8Array {
    if (!this.k) return data;
    const out = chacha20poly1305(this.k, nonce(this.n), ad).decrypt(data);
    this.n += 1n;
    return out;
  }
}

class SymmetricState {
  ck: Uint8Array;
  h: Uint8Array;
  cs = new CipherState();

  constructor() {
    const name = new TextEncoder().encode(PROTOCOL);
    if (name.length <= HASHLEN) {
      this.h = new Uint8Array(HASHLEN);
      this.h.set(name);
    } else {
      this.h = hash(name);
    }
    this.ck = this.h;
  }

  mixKey(ikm: Uint8Array) {
    const [ck, k] = hkdf(this.ck, ikm, 2);
    this.ck = ck;
    this.cs = new CipherState(k);
  }

  mixHash(data: Uint8Array) {
    this.h = hash(concat(this.h, data));
  }

  encryptAndHash(plain: Uint8Array): Uint8Array {
    const c = this.cs.encrypt(this.h, plain);
    this.mixHash(c);
    return c;
  }

  decryptAndHash(data: Uint8Array): Uint8Array {
    const p = this.cs.decrypt(this.h, data);
    this.mixHash(data);
    return p;
  }

  /** Initiator-to-responder cipher first, then responder-to-initiator. */
  split(): [CipherState, CipherState] {
    const [k1, k2] = hkdf(this.ck, new Uint8Array(0), 2);
    return [new CipherState(k1), new CipherState(k2)];
  }
}

export interface Keypair {
  private: Uint8Array;
  public: Uint8Array;
}

export function keypairFromPrivate(priv: Uint8Array): Keypair {
  if (priv.length !== DHLEN) throw new Error("a key is 32 bytes");
  return { private: priv, public: x25519.getPublicKey(priv) };
}

export interface Transport {
  /** Decrypts what the initiator sends. */
  recv: CipherState;
  /** Encrypts what this side sends. */
  send: CipherState;
  /** The initiator's static public key, proven by the handshake. */
  peer: Uint8Array;
}

/**
 * Reads the initiator's first message. Returns its payload, the initiator's proven
 * static key, and a function that writes our answer (with its payload) and yields the
 * transport ciphers. Throws if the message was not encrypted to our key.
 */
export function respond(
  me: Keypair,
  message: Uint8Array,
  ephemeral: Uint8Array = x25519.utils.randomSecretKey(),
): {
  payload: Uint8Array;
  peer: Uint8Array;
  answer: (payload: Uint8Array) => { message: Uint8Array; transport: Transport };
} {
  const ss = new SymmetricState();
  ss.mixHash(PROLOGUE);
  ss.mixHash(me.public); // <- s (pre-message)

  // -> e, es, s, ss
  if (message.length < DHLEN + DHLEN + TAG + TAG) throw new Error("handshake message too short");
  let at = 0;
  const re = message.subarray(at, (at += DHLEN));
  ss.mixHash(re);
  ss.mixKey(x25519.getSharedSecret(me.private, re)); // es
  const rs = ss.decryptAndHash(message.subarray(at, (at += DHLEN + TAG)));
  ss.mixKey(x25519.getSharedSecret(me.private, rs)); // ss
  const payload = ss.decryptAndHash(message.subarray(at));

  return {
    payload,
    peer: rs,
    answer(out: Uint8Array) {
      // <- e, ee, se
      const e = keypairFromPrivate(ephemeral);
      ss.mixHash(e.public);
      ss.mixKey(x25519.getSharedSecret(e.private, re)); // ee
      ss.mixKey(x25519.getSharedSecret(e.private, rs)); // se
      const body = ss.encryptAndHash(out);
      const [c1, c2] = ss.split();
      return {
        message: concat(e.public, body),
        transport: { recv: c1, send: c2, peer: rs },
      };
    },
  };
}

export const b64 = {
  encode: (b: Uint8Array) => btoa(String.fromCharCode(...b)),
  decode: (s: string) => Uint8Array.from(atob(s.trim()), (c) => c.charCodeAt(0)),
};
