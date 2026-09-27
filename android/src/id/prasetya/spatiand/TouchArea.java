package id.prasetya.spatiand;

import android.content.Context;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

/**
 * The phone's touch area: the Deck's left pad, its ground drawn by the session
 * (crates/spatiand-android/src/android/panel.rs).
 *
 * Touches go to the session raw, in the area's own 0..1 coordinates; what they mean -- scroll,
 * tap, hold, long press, pinch -- is decided there (spatiand-input's phone.rs), where it is
 * tested. It is also where Android's own keyboard types: what it commits goes to the session as
 * text, and Enter and backspace as keys.
 */
public class TouchArea extends SurfaceView implements SurfaceHolder.Callback {
    public TouchArea(Context context) {
        super(context);
        getHolder().addCallback(this);
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    @Override
    public boolean onTouchEvent(MotionEvent e) {
        float w = Math.max(1, getWidth()), h = Math.max(1, getHeight());
        int action = e.getActionMasked();
        switch (action) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP: {
                int i = e.getActionIndex();
                int phase = (action == MotionEvent.ACTION_DOWN || action == MotionEvent.ACTION_POINTER_DOWN) ? 0 : 2;
                Native.touch(phase, e.getPointerId(i), e.getX(i) / w, e.getY(i) / h);
                break;
            }
            case MotionEvent.ACTION_MOVE:
                for (int i = 0; i < e.getPointerCount(); i++) {
                    Native.touch(1, e.getPointerId(i), e.getX(i) / w, e.getY(i) / h);
                }
                break;
            case MotionEvent.ACTION_CANCEL:
                for (int i = 0; i < e.getPointerCount(); i++) {
                    Native.touch(3, e.getPointerId(i), e.getX(i) / w, e.getY(i) / h);
                }
                break;
            default:
                break;
        }
        return true;
    }

    /** Bring Android's keyboard up, or put it away if it is up. */
    public void toggleKeyboard() {
        InputMethodManager ime = getContext().getSystemService(InputMethodManager.class);
        android.view.WindowInsets insets = getRootWindowInsets();
        boolean up = insets != null && insets.isVisible(android.view.WindowInsets.Type.ime());
        if (up) {
            ime.hideSoftInputFromWindow(getWindowToken(), 0);
        } else {
            requestFocus();
            ime.showSoftInput(this, 0);
        }
    }

    /**
     * What a keyboard that composes has shown as the word in progress, and already sent.
     *
     * Every character goes to the session as it is typed: a terminal on the far side shows
     * nothing it has not been sent, and a keyboard that holds the word until a space or the
     * next key made the first letter appear late or not at all. So the composing text is sent
     * as it changes -- only the difference, with backspaces for what was taken back -- and a
     * commit sends whatever of it is still new.
     */
    private String composing = "";

    private void sendDifference(String before, String after) {
        int same = 0;
        while (same < before.length() && same < after.length()
                && before.charAt(same) == after.charAt(same)) {
            same++;
        }
        for (int i = same; i < before.length(); i++) {
            Native.key(KeyEvent.KEYCODE_DEL, true);
            Native.key(KeyEvent.KEYCODE_DEL, false);
        }
        if (same < after.length()) Native.text(after.substring(same));
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo info) {
        // Plain text with no suggestions: every character goes straight out as it is typed,
        // since there is nothing here to hold a word while it is being composed.
        // A visible password is what keyboards take as "no word to compose, no corrections":
        // keys arrive one at a time, as a terminal wants them.
        info.inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD;
        info.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_FULLSCREEN;
        composing = "";
        return new BaseInputConnection(this, false) {
            @Override
            public boolean commitText(CharSequence text, int newCursorPosition) {
                sendDifference(composing, text.toString());
                composing = "";
                return true;
            }

            @Override
            public boolean setComposingText(CharSequence text, int newCursorPosition) {
                String now = text.toString();
                sendDifference(composing, now);
                composing = now;
                return true;
            }

            @Override
            public boolean finishComposingText() {
                // What was composed has been sent already, and stays.
                composing = "";
                return true;
            }

            @Override
            public boolean deleteSurroundingText(int before, int after) {
                composing = "";
                for (int i = 0; i < before; i++) {
                    Native.key(KeyEvent.KEYCODE_DEL, true);
                    Native.key(KeyEvent.KEYCODE_DEL, false);
                }
                return true;
            }

            @Override
            public boolean sendKeyEvent(KeyEvent event) {
                Native.key(event.getKeyCode(), event.getAction() == KeyEvent.ACTION_DOWN);
                return true;
            }

            @Override
            public boolean performEditorAction(int action) {
                Native.key(KeyEvent.KEYCODE_ENTER, true);
                Native.key(KeyEvent.KEYCODE_ENTER, false);
                return true;
            }
        };
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        Native.phoneSurface(holder.getSurface());
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        // A new size is a new surface to the session.
        Native.phoneSurface(holder.getSurface());
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        Native.phoneSurfaceGone();
    }
}
