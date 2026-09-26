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
 * The phone's touch area: the Deck's left pad, with the machine's monitors drawn under it by
 * the session (crates/spatiand-android/src/android/panel.rs).
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

    /** Bring Android's keyboard up, or put it away. */
    public void toggleKeyboard() {
        InputMethodManager ime = getContext().getSystemService(InputMethodManager.class);
        requestFocus();
        if (!ime.isActive(this) || !isFocused()) {
            ime.showSoftInput(this, 0);
        } else {
            ime.toggleSoftInput(0, 0);
        }
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo info) {
        // Plain text with no suggestions: every character goes straight out as it is typed,
        // since there is nothing here to hold a word while it is being composed.
        info.inputType = InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        info.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI | EditorInfo.IME_FLAG_NO_FULLSCREEN;
        return new BaseInputConnection(this, false) {
            @Override
            public boolean commitText(CharSequence text, int newCursorPosition) {
                Native.text(text.toString());
                return true;
            }

            @Override
            public boolean setComposingText(CharSequence text, int newCursorPosition) {
                // A keyboard that insists on composing: take the word when it is committed.
                return true;
            }

            @Override
            public boolean deleteSurroundingText(int before, int after) {
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
