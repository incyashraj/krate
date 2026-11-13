package tech.krate.player;

import android.content.Context;
import android.text.Editable;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

import java.util.ArrayList;

/**
 * The phone's on-screen keyboard for Krate's apps (K-1024).
 *
 * A NativeActivity has no text editor, so no keyboard ever came up and no
 * Krate app could be typed into on a phone without a hardware keyboard. This
 * is one invisible view that says it is a text editor; the keyboard talks to
 * its input connection, and every call is written down as one short line for
 * Krate's thread to read (krateTyped), which turns the lines into the key
 * presses and typed text the app hears (crates/adapter-android typing.rs):
 *
 *   c:text   committed text          m:text  the word being composed
 *   r:text   text composed again     f:      composing finished
 *   d:b,a    delete b before, a after the caret
 *   k:Name   a named key (Enter, Backspace, ArrowLeft...)
 */
final class Typing extends View {
    private final ArrayList<String> heard = new ArrayList<>();
    /** A field of many lines: Enter makes a new line rather than Done. */
    private boolean manyLines;
    private Connection connection;

    Typing(Context context) {
        super(context);
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo info) {
        info.inputType = InputType.TYPE_CLASS_TEXT
                | InputType.TYPE_TEXT_FLAG_AUTO_CORRECT
                | InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
                | (manyLines ? InputType.TYPE_TEXT_FLAG_MULTI_LINE : 0);
        // Never the full-screen editor a landscape keyboard offers: the app's
        // own field is where the person watches what they type.
        info.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN | EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | (manyLines ? EditorInfo.IME_ACTION_NONE : EditorInfo.IME_ACTION_DONE);
        info.initialSelStart = 0;
        info.initialSelEnd = 0;
        connection = new Connection(this);
        return connection;
    }

    /** Up for a field (one line or many), or away. True when asked up. */
    boolean set(boolean on, boolean many) {
        InputMethodManager keyboard =
                (InputMethodManager) getContext().getSystemService(Context.INPUT_METHOD_SERVICE);
        if (keyboard == null) {
            return false;
        }
        if (on) {
            manyLines = many;
            synchronized (heard) {
                heard.clear();
            }
            // A new field starts empty for the keyboard: what it remembers
            // of the last one must not be rewritten into this one.
            if (connection != null) {
                Editable kept = connection.getEditable();
                if (kept != null) {
                    kept.clear();
                }
            }
            requestFocus();
            keyboard.restartInput(this);
            final InputMethodManager asked = keyboard;
            // Asked again once the focus has settled: a view focused a moment
            // ago is not always the one the keyboard will serve yet.
            post(new Runnable() {
                @Override
                public void run() {
                    if (hasFocus()) {
                        asked.showSoftInput(Typing.this, 0);
                    }
                }
            });
            return keyboard.showSoftInput(this, 0) || hasFocus();
        }
        keyboard.hideSoftInputFromWindow(getWindowToken(), 0);
        clearFocus();
        return false;
    }

    /** What the keyboard did since the last ask. */
    String[] take() {
        synchronized (heard) {
            String[] all = heard.toArray(new String[0]);
            heard.clear();
            return all;
        }
    }

    private void hear(String line) {
        synchronized (heard) {
            // A keyboard left talking to an app that never reads is bounded.
            if (heard.size() < 4096) {
                heard.add(line);
            }
        }
    }

    /** The key's name as Krate spells it, or null for one it has no name for. */
    static String keyName(int code) {
        switch (code) {
            case KeyEvent.KEYCODE_DEL: return "Backspace";
            case KeyEvent.KEYCODE_FORWARD_DEL: return "Delete";
            case KeyEvent.KEYCODE_ENTER:
            case KeyEvent.KEYCODE_NUMPAD_ENTER: return "Enter";
            case KeyEvent.KEYCODE_TAB: return "Tab";
            case KeyEvent.KEYCODE_ESCAPE: return "Escape";
            case KeyEvent.KEYCODE_DPAD_LEFT: return "ArrowLeft";
            case KeyEvent.KEYCODE_DPAD_RIGHT: return "ArrowRight";
            case KeyEvent.KEYCODE_DPAD_UP: return "ArrowUp";
            case KeyEvent.KEYCODE_DPAD_DOWN: return "ArrowDown";
            case KeyEvent.KEYCODE_MOVE_HOME: return "Home";
            case KeyEvent.KEYCODE_MOVE_END: return "End";
            case KeyEvent.KEYCODE_PAGE_UP: return "PageUp";
            case KeyEvent.KEYCODE_PAGE_DOWN: return "PageDown";
            default: return null;
        }
    }

    /**
     * The keyboard's side of the field. It keeps its own copy of what was
     * typed (a full editor), so the keyboard's suggestions have the word to
     * work on, and writes each change down for Krate.
     */
    private final class Connection extends BaseInputConnection {
        Connection(View view) {
            super(view, true);
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            hear("c:" + text);
            return super.commitText(text, newCursorPosition);
        }

        @Override
        public boolean setComposingText(CharSequence text, int newCursorPosition) {
            hear("m:" + text);
            return super.setComposingText(text, newCursorPosition);
        }

        @Override
        public boolean setComposingRegion(int start, int end) {
            Editable kept = getEditable();
            if (kept != null) {
                int from = Math.max(0, Math.min(Math.min(start, end), kept.length()));
                int to = Math.max(0, Math.min(Math.max(start, end), kept.length()));
                hear("r:" + kept.subSequence(from, to));
            }
            return super.setComposingRegion(start, end);
        }

        @Override
        public boolean finishComposingText() {
            hear("f:");
            return super.finishComposingText();
        }

        @Override
        public boolean deleteSurroundingText(int before, int after) {
            hear("d:" + Math.max(0, before) + "," + Math.max(0, after));
            return super.deleteSurroundingText(before, after);
        }

        @Override
        public boolean deleteSurroundingTextInCodePoints(int before, int after) {
            hear("d:" + Math.max(0, before) + "," + Math.max(0, after));
            return super.deleteSurroundingTextInCodePoints(before, after);
        }

        @Override
        public boolean performEditorAction(int action) {
            hear("k:Enter");
            return true;
        }

        /**
         * Keys the keyboard presses rather than types (Backspace on an empty
         * field, arrows, Enter). Written down here and never passed on, so a
         * key is not heard twice through the activity's own input queue.
         */
        @Override
        public boolean sendKeyEvent(KeyEvent event) {
            if (event.getAction() == KeyEvent.ACTION_DOWN) {
                String name = keyName(event.getKeyCode());
                if (name != null) {
                    hear("k:" + name);
                } else {
                    int typed = event.getUnicodeChar(event.getMetaState());
                    if (typed > 0) {
                        hear("c:" + new String(Character.toChars(typed)));
                    }
                }
            }
            return true;
        }
    }
}
