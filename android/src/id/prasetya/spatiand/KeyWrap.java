package id.prasetya.spatiand;

import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.KeyFactory;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECParameterSpec;
import java.security.spec.ECPoint;
import java.security.spec.ECPublicKeySpec;
import java.util.Arrays;

import javax.crypto.Cipher;
import javax.crypto.Mac;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * SpatiKey's wrap of the vault key to a holder's key (docs/spatikey.md, "Keys"): the very format
 * of the Mac's SpatiKeyWrap and of crates/spatikey-core/src/holders.rs. An ephemeral P-256 key,
 * ECDH with the holder's key, HKDF-SHA256 (salt "spatikey", info = ephemeral public, then the
 * holder's, both X9.63), AES-256-GCM with the associated data "spatikey touch-id v1"; kept as
 * ephemeral public (65) | nonce (12) | vault key (32) | tag (16).
 *
 * Only opening is here: the ECDH is the Keystore's, behind the finger (KeyGuard), and what it
 * gives is opened with plain javax.crypto, so that this class can be checked on any JVM.
 */
final class KeyWrap {
    static final int PUBLIC = 65;
    static final int NONCE = 12;
    static final int LENGTH = PUBLIC + NONCE + 32 + 16;
    private static final byte[] SALT = "spatikey".getBytes(StandardCharsets.US_ASCII);
    private static final byte[] AAD = "spatikey touch-id v1".getBytes(StandardCharsets.US_ASCII);

    private KeyWrap() {}

    /** An EC public key as X9.63: 4, then x and y, 32 bytes each. */
    static byte[] x963(ECPublicKey key) {
        byte[] out = new byte[PUBLIC];
        out[0] = 4;
        put(key.getW().getAffineX(), out, 1);
        put(key.getW().getAffineY(), out, 33);
        return out;
    }

    private static void put(BigInteger n, byte[] out, int at) {
        byte[] bytes = n.toByteArray();
        int length = Math.min(bytes.length, 32);
        System.arraycopy(bytes, bytes.length - length, out, at + 32 - length, length);
    }

    /** The wrap's ephemeral key, on the curve of `params`. Throws for a wrap of another shape. */
    static ECPublicKey ephemeral(byte[] wrapped, ECParameterSpec params) throws GeneralSecurityException {
        if (wrapped.length != LENGTH || wrapped[0] != 4) {
            throw new GeneralSecurityException("not a wrap of SpatiKey's");
        }
        BigInteger x = new BigInteger(1, Arrays.copyOfRange(wrapped, 1, 33));
        BigInteger y = new BigInteger(1, Arrays.copyOfRange(wrapped, 33, PUBLIC));
        return (ECPublicKey) KeyFactory.getInstance("EC").generatePublic(new ECPublicKeySpec(new ECPoint(x, y), params));
    }

    /**
     * The vault key from `wrapped`, given the ECDH's shared secret with its ephemeral key and the
     * holder's own public key (X9.63). The caller overwrites what comes back once it is used.
     */
    static byte[] open(byte[] shared, byte[] recipient, byte[] wrapped) throws GeneralSecurityException {
        if (wrapped.length != LENGTH) throw new GeneralSecurityException("not a wrap of SpatiKey's");
        byte[] info = new byte[PUBLIC + recipient.length];
        System.arraycopy(wrapped, 0, info, 0, PUBLIC);
        System.arraycopy(recipient, 0, info, PUBLIC, recipient.length);
        byte[] key = hkdf(shared, info);
        try {
            Cipher gcm = Cipher.getInstance("AES/GCM/NoPadding");
            gcm.init(Cipher.DECRYPT_MODE, new SecretKeySpec(key, "AES"), new GCMParameterSpec(128, wrapped, PUBLIC, NONCE));
            gcm.updateAAD(AAD);
            return gcm.doFinal(wrapped, PUBLIC + NONCE, 32 + 16);
        } finally {
            Arrays.fill(key, (byte) 0);
        }
    }

    /** HKDF-SHA256 (RFC 5869) with SpatiKey's salt, for one 32-byte key. */
    private static byte[] hkdf(byte[] ikm, byte[] info) throws GeneralSecurityException {
        Mac mac = Mac.getInstance("HmacSHA256");
        mac.init(new SecretKeySpec(SALT, "HmacSHA256"));
        byte[] prk = mac.doFinal(ikm);
        try {
            mac.init(new SecretKeySpec(prk, "HmacSHA256"));
            mac.update(info);
            mac.update((byte) 1);
            return mac.doFinal();
        } finally {
            Arrays.fill(prk, (byte) 0);
        }
    }
}
