package id.prasetya.spatiand;

import android.app.AlertDialog;
import android.content.Context;
import android.content.pm.PackageManager;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.os.Build;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyPermanentlyInvalidatedException;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import android.util.Log;
import android.view.WindowManager;

import org.json.JSONObject;

import java.nio.charset.StandardCharsets;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.SecureRandom;
import java.security.cert.Certificate;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECGenParameterSpec;
import java.util.Arrays;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

import javax.crypto.AEADBadTagException;
import javax.crypto.KeyAgreement;

/**
 * SpatiKey's half on an Android device with a strong (class 3) biometric: what only the app can
 * do for SpatiKey's service (docs/spatikey.md, "The holder's half"), as SpatiKeyGuard.swift does
 * it on a Mac. crates/spatiand-android/src/android/key.rs asks, from the service's threads; each
 * question carries a number, and its answer goes back with it through answer() or answerKey().
 *
 * The key is an Android Keystore P-256 key for key agreement, made for the biometrics enrolled
 * then (adding one retires it, and the recovery code sets this device up again) and usable only
 * after a strong biometric: on Android 15 and later for that one use (the ECDH is the prompt's
 * CryptoObject), before that for a few seconds after the finger (KeyAgreement could not be a
 * CryptoObject until Android 15). No device credential stands in for the finger. The vault key is
 * wrapped to the key by whichever device adds this one (crates/spatikey-core/src/holders.rs), and
 * is opened here by an ECDH inside the Keystore -- which is what asks for the finger -- and
 * KeyWrap.
 *
 * The blob the service keeps for the key (local/spatikey/touch-id.json) is the Keystore alias and
 * how it was made, as JSON in base64: usable by this app on this device only.
 */
final class KeyGuard {
    /** The service's questions: spatikey-core's platform::Asked. */
    static final int UNLOCK = 1;
    static final int ENROLL = 2;
    static final int CAN_TOUCH = 3;
    static final int ALLOW = 4;
    static final int MAKE = 5;

    private static final String TAG = "SpatiKey";
    /** The service gives up after 120 s for a finger and 180 s for the owner: the prompt and the
     *  question go a little before, so that nothing is answered that nobody hears. */
    private static final long TOUCH_WAIT_MS = 110_000;
    private static final long ALLOW_WAIT_MS = 170_000;
    /** How long a key made before Android 15 may be used after the finger. */
    private static final int AFTER_FINGER_SECS = 5;
    private static final String KEYSTORE = "AndroidKeyStore";

    /** The Keystore's work, one at a time, off the main thread. */
    private static final ExecutorService work = Executors.newSingleThreadExecutor();
    private static final Handler main = new Handler(Looper.getMainLooper());

    private KeyGuard() {}

    /** The answer to question `id`, as JSON. */
    static native void answer(long id, String json);

    /** An unlock's answer: the vault key, overwritten here once this returns. */
    static native void answerKey(long id, byte[] vk);

    /**
     * Whether this device can hold SpatiKey's vault, asking nobody: -1 while the app has not
     * started, 0 no biometric hardware (the Beam Pro), 1 there but nothing strong enrolled or not
     * usable now, 2 a strong biometric ready.
     */
    static int sensor() {
        Context c = Shell.context;
        if (c == null) return -1;
        BiometricManager biometrics = c.getSystemService(BiometricManager.class);
        if (biometrics == null) return 0;
        int can = biometrics.canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG);
        if (can == BiometricManager.BIOMETRIC_SUCCESS) return 2;
        PackageManager packages = c.getPackageManager();
        boolean hardware = packages.hasSystemFeature(PackageManager.FEATURE_FINGERPRINT)
                || packages.hasSystemFeature(PackageManager.FEATURE_FACE)
                || packages.hasSystemFeature(PackageManager.FEATURE_IRIS);
        if (can == BiometricManager.BIOMETRIC_ERROR_NO_HARDWARE || !hardware) return 0;
        return 1;
    }

    /** A question from the service: returns at once; the answer follows. */
    static void ask(long id, int what, String json) {
        switch (what) {
            case CAN_TOUCH:
                answer(id, "{\"ok\":" + (sensor() == 2) + "}");
                return;
            case MAKE:
                work.execute(() -> make(id));
                return;
            case UNLOCK:
                work.execute(() -> unlock(id, json));
                return;
            case ALLOW:
                main.post(() -> allow(id, json));
                return;
            default:
                // ENROLL is the Mac's: here the key is made (MAKE) and the vault key wrapped to
                // it by the service, so that it never sits in a Java string.
                answer(id, refused("failed", "SpatiKey asked this device for something it does not do (" + what + ")"));
        }
    }

    // --- making the key ---

    private static void make(long id) {
        if (sensor() != 2) {
            answer(id, refused("touch_id_unavailable", "no fingerprint or face is enrolled as a strong biometric on this device"));
            return;
        }
        boolean perUse = Build.VERSION.SDK_INT >= 35;
        byte[] random = new byte[8];
        new SecureRandom().nextBytes(random);
        String alias = "spatikey-" + hex(random);
        try {
            ECPublicKey made = null;
            try {
                made = generate(alias, perUse, true);
            } catch (Exception strongBox) {
                // StrongBox is not on every device, and not every StrongBox agrees keys: the TEE.
                Log.i(TAG, "no StrongBox key (" + strongBox.getClass().getSimpleName() + "); the TEE's");
                made = generate(alias, perUse, false);
            }
            JSONObject blob = new JSONObject().put("alias", alias).put("per_use", perUse);
            JSONObject out = new JSONObject()
                    .put("public", Base64.encodeToString(KeyWrap.x963(made), Base64.NO_WRAP))
                    .put("blob", Base64.encodeToString(blob.toString().getBytes(StandardCharsets.UTF_8), Base64.NO_WRAP));
            answer(id, out.toString());
        } catch (Exception e) {
            Log.w(TAG, "no key was made", e);
            answer(id, refused("failed", "the Keystore would not make a key (" + e.getClass().getSimpleName() + ")"));
        }
    }

    private static ECPublicKey generate(String alias, boolean perUse, boolean strongBox) throws Exception {
        KeyGenParameterSpec.Builder spec = new KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_AGREE_KEY)
                .setAlgorithmParameterSpec(new ECGenParameterSpec("secp256r1"))
                .setUserAuthenticationRequired(true)
                .setUserAuthenticationParameters(perUse ? 0 : AFTER_FINGER_SECS, KeyProperties.AUTH_BIOMETRIC_STRONG)
                .setInvalidatedByBiometricEnrollment(true)
                .setIsStrongBoxBacked(strongBox);
        KeyPairGenerator generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, KEYSTORE);
        generator.initialize(spec.build());
        return (ECPublicKey) generator.generateKeyPair().getPublic();
    }

    // --- opening the vault key ---

    private static void unlock(long id, String json) {
        byte[] wrapped;
        String alias;
        boolean perUse;
        String reason;
        try {
            JSONObject question = new JSONObject(json);
            reason = sentence(question.optString("reason"));
            wrapped = Base64.decode(question.getString("wrapped"), Base64.DEFAULT);
            JSONObject blob = new JSONObject(new String(Base64.decode(question.getString("blob"), Base64.DEFAULT), StandardCharsets.UTF_8));
            alias = blob.getString("alias");
            perUse = blob.optBoolean("per_use") && Build.VERSION.SDK_INT >= 35;
        } catch (Exception e) {
            answer(id, refused("failed", "SpatiKey's question for the fingerprint sensor could not be read"));
            return;
        }
        if (wrapped.length != KeyWrap.LENGTH) {
            answer(id, refused("failed", "SpatiKey's key file is damaged: the recovery code sets this device up again"));
            return;
        }
        final PrivateKey key;
        final ECPublicKey mine;
        final ECPublicKey ephemeral;
        try {
            KeyStore store = KeyStore.getInstance(KEYSTORE);
            store.load(null);
            key = (PrivateKey) store.getKey(alias, null);
            Certificate certificate = store.getCertificate(alias);
            if (key == null || certificate == null) {
                answer(id, refused("fingerprints_changed", "this device's key for SpatiKey is gone: the recovery code sets it up again"));
                return;
            }
            mine = (ECPublicKey) certificate.getPublicKey();
            ephemeral = KeyWrap.ephemeral(wrapped, mine.getParams());
        } catch (Exception e) {
            answer(id, refused("failed", "the Keystore could not be read (" + e.getClass().getSimpleName() + ")"));
            return;
        }
        try {
            if (perUse) {
                KeyAgreement agreement = KeyAgreement.getInstance("ECDH", KEYSTORE);
                agreement.init(key);
                prompt(id, reason, new BiometricPrompt.CryptoObject(agreement),
                        result -> finish(id, result.getCryptoObject().getKeyAgreement(), ephemeral, mine, wrapped));
            } else {
                prompt(id, reason, null, result -> {
                    try {
                        KeyAgreement agreement = KeyAgreement.getInstance("ECDH", KEYSTORE);
                        agreement.init(key);
                        finish(id, agreement, ephemeral, mine, wrapped);
                    } catch (Exception e) {
                        answer(id, failure(e));
                    }
                });
            }
        } catch (Exception e) {
            answer(id, failure(e));
        }
    }

    /** The ECDH, now that the finger has been given, and the vault key opened with it. */
    private static void finish(long id, KeyAgreement agreement, ECPublicKey ephemeral, ECPublicKey mine, byte[] wrapped) {
        byte[] shared = null;
        byte[] vk = null;
        try {
            agreement.doPhase(ephemeral, true);
            shared = agreement.generateSecret();
            vk = KeyWrap.open(shared, KeyWrap.x963(mine), wrapped);
            answerKey(id, vk);
        } catch (AEADBadTagException e) {
            answer(id, refused("failed", "the key opened, but the vault key in SpatiKey's file did not open with it"));
        } catch (Exception e) {
            answer(id, failure(e));
        } finally {
            if (shared != null) Arrays.fill(shared, (byte) 0);
            if (vk != null) Arrays.fill(vk, (byte) 0);
        }
    }

    private interface Then {
        void run(BiometricPrompt.AuthenticationResult result);
    }

    /** BiometricPrompt, strong biometrics only, with no device credential standing in. */
    private static void prompt(long id, String reason, BiometricPrompt.CryptoObject crypto, Then then) {
        Context c = Shell.context;
        if (c == null) {
            answer(id, refused("touch_id_unavailable", "Spatiand is not running"));
            return;
        }
        main.post(() -> {
            AtomicBoolean done = new AtomicBoolean();
            CancellationSignal cancel = new CancellationSignal();
            Runnable late = () -> {
                if (done.compareAndSet(false, true)) {
                    cancel.cancel();
                    answer(id, refused("timeout", "no finger was put on the sensor in time"));
                }
            };
            main.postDelayed(late, TOUCH_WAIT_MS);
            BiometricPrompt.AuthenticationCallback callback = new BiometricPrompt.AuthenticationCallback() {
                @Override
                public void onAuthenticationSucceeded(BiometricPrompt.AuthenticationResult result) {
                    if (!done.compareAndSet(false, true)) return;
                    main.removeCallbacks(late);
                    work.execute(() -> then.run(result));
                }

                @Override
                public void onAuthenticationError(int code, CharSequence text) {
                    if (!done.compareAndSet(false, true)) return;
                    main.removeCallbacks(late);
                    answer(id, refusedFor(code, text));
                }
            };
            try {
                BiometricPrompt prompt = new BiometricPrompt.Builder(c)
                        .setTitle("SpatiKey")
                        .setSubtitle("Spatiand is trying to " + reason)
                        .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
                        .setConfirmationRequired(false)
                        .setNegativeButton("Cancel", c.getMainExecutor(), (dialog, which) -> {
                            if (done.compareAndSet(false, true)) {
                                main.removeCallbacks(late);
                                answer(id, refused("denied", "the fingerprint prompt was cancelled"));
                            }
                        })
                        .build();
                if (crypto != null) {
                    prompt.authenticate(crypto, cancel, c.getMainExecutor(), callback);
                } else {
                    prompt.authenticate(cancel, c.getMainExecutor(), callback);
                }
            } catch (Exception e) {
                if (done.compareAndSet(false, true)) {
                    main.removeCallbacks(late);
                    answer(id, failure(e));
                }
            }
        });
    }

    /** BiometricPrompt's error, in the service's words. */
    private static String refusedFor(int code, CharSequence text) {
        String said = text == null ? "" : text.toString();
        switch (code) {
            case BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED:
            case BiometricPrompt.BIOMETRIC_ERROR_CANCELED:
                return refused("denied", "the fingerprint prompt was cancelled");
            case BiometricPrompt.BIOMETRIC_ERROR_TIMEOUT:
                return refused("timeout", "no finger was put on the sensor in time");
            case BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT:
            case BiometricPrompt.BIOMETRIC_ERROR_LOCKOUT_PERMANENT:
                return refused("touch_id_unavailable", "the sensor is locked after too many tries: unlock the device once, then try again");
            case BiometricPrompt.BIOMETRIC_ERROR_HW_UNAVAILABLE:
            case BiometricPrompt.BIOMETRIC_ERROR_HW_NOT_PRESENT:
            case BiometricPrompt.BIOMETRIC_ERROR_NO_BIOMETRICS:
                return refused("touch_id_unavailable", "the fingerprint sensor cannot be used now (" + said + ")");
            default:
                return refused("failed", "the fingerprint prompt failed (" + code + ": " + said + ")");
        }
    }

    /** A Keystore failure, in the service's words: a key retired by a change of fingers says so. */
    private static String failure(Exception e) {
        if (e instanceof KeyPermanentlyInvalidatedException) {
            return refused("fingerprints_changed",
                    "the fingerprints enrolled on this device changed since SpatiKey was set up on it: the recovery code sets it up again");
        }
        Log.w(TAG, "the vault key was not opened", e);
        return refused("failed", "the Keystore could not open the vault key (" + e.getClass().getSimpleName() + ")");
    }

    // --- letting a device ask this one ---

    /** The owner's yes or no to a device that wants to ask this one, with the six digits both show. */
    private static void allow(long id, String json) {
        Context c = Shell.context;
        String name;
        String code;
        try {
            JSONObject question = new JSONObject(json);
            name = shown(question.optString("name"));
            code = question.optString("code");
        } catch (Exception e) {
            answer(id, "{\"allow\":false}");
            return;
        }
        if (c == null || code.isEmpty()) {
            answer(id, "{\"allow\":false}");
            return;
        }
        AtomicBoolean done = new AtomicBoolean();
        try {
            AlertDialog[] shown = new AlertDialog[1];
            Runnable no = () -> {
                if (done.compareAndSet(false, true)) answer(id, "{\"allow\":false}");
            };
            shown[0] = new AlertDialog.Builder(c, android.R.style.Theme_DeviceDefault_Dialog_Alert)
                    .setTitle("“" + name + "” wants to use SpatiKey on this device")
                    .setMessage("Make sure it shows " + spaced(code) + ". Each use still needs your finger here.")
                    .setPositiveButton("Allow", (dialog, which) -> {
                        if (done.compareAndSet(false, true)) answer(id, "{\"allow\":true}");
                    })
                    .setNegativeButton("Don’t Allow", (dialog, which) -> no.run())
                    .setOnCancelListener(dialog -> no.run())
                    .create();
            // Over whatever is on the phone's screen: the app may draw over other apps (the glasses' window).
            shown[0].getWindow().setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY);
            shown[0].show();
            main.postDelayed(() -> {
                if (shown[0].isShowing()) shown[0].dismiss();
                no.run();
            }, ALLOW_WAIT_MS);
        } catch (Exception e) {
            Log.w(TAG, "could not ask about a device", e);
            if (done.compareAndSet(false, true)) answer(id, "{\"allow\":false}");
        }
    }

    // --- words ---

    private static String refused(String code, String message) {
        try {
            return new JSONObject().put("error", code).put("message", message).toString();
        } catch (Exception e) {
            return "{\"error\":\"failed\",\"message\":\"\"}";
        }
    }

    /** What finishes "Spatiand is trying to ...": one line, not too long, never empty. */
    private static String sentence(String reason) {
        String line = reason.replaceAll("\\p{Cntrl}", " ").trim();
        if (line.isEmpty()) return "open SpatiKey";
        return line.length() > 160 ? line.substring(0, 159) + "…" : line;
    }

    /** A device's name as it gives it, fit for one line. */
    private static String shown(String name) {
        String line = name.replaceAll("\\p{Cntrl}", " ").trim();
        if (line.isEmpty()) return "A device";
        return line.length() > 40 ? line.substring(0, 39) + "…" : line;
    }

    /** Six digits as both ends show them: "123 456". */
    private static String spaced(String code) {
        String digits = code.replaceAll("[^0-9]", "");
        return digits.length() == 6 ? digits.substring(0, 3) + " " + digits.substring(3) : code;
    }

    private static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder();
        for (byte b : bytes) out.append(String.format("%02x", b));
        return out.toString();
    }
}
