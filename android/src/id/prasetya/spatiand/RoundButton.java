package id.prasetya.spatiand;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.view.View;

/**
 * A round button under the thumb, with its icon drawn rather than written: the bar's buttons
 * are pressed without looking, and a glyph from whatever font the phone has is a different
 * size and weight on every phone.
 */
public class RoundButton extends View {
    enum Icon { BACK, SETTINGS, MENU, KEYBOARD, KEYBOARD_HIDE }

    private Icon icon;
    private final Paint disc = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint ink = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path path = new Path();
    private final RectF box = new RectF();

    public RoundButton(Context context, Icon icon) {
        super(context);
        this.icon = icon;
        ink.setColor(Color.rgb(222, 228, 240));
        ink.setStrokeCap(Paint.Cap.ROUND);
        ink.setStrokeJoin(Paint.Join.ROUND);
        setClickable(true);
        setContentDescription(icon.name().toLowerCase());
    }

    void setIcon(Icon icon) {
        if (this.icon != icon) {
            this.icon = icon;
            invalidate();
        }
    }

    @Override
    public void setPressed(boolean pressed) {
        super.setPressed(pressed);
        invalidate();
    }

    @Override
    protected void onMeasure(int widthSpec, int heightSpec) {
        int size = (int) (64 * getResources().getDisplayMetrics().density);
        setMeasuredDimension(resolveSize(size, widthSpec), resolveSize(size, heightSpec));
    }

    @Override
    protected void onDraw(Canvas c) {
        float cx = getWidth() / 2f, cy = getHeight() / 2f;
        float r = Math.min(cx, cy) - 2;
        disc.setColor(isPressed() ? Color.rgb(78, 88, 110) : Color.rgb(38, 43, 56));
        c.drawCircle(cx, cy, r, disc);
        // The icon lives in a square of this half-size, centred.
        float s = r * 0.42f;
        ink.setStrokeWidth(r * 0.1f);
        ink.setStyle(Paint.Style.STROKE);
        path.reset();
        switch (icon) {
            case BACK:
                // An arrow pointing left.
                path.moveTo(cx + s, cy);
                path.lineTo(cx - s, cy);
                path.moveTo(cx - s * 0.2f, cy - s * 0.8f);
                path.lineTo(cx - s, cy);
                path.lineTo(cx - s * 0.2f, cy + s * 0.8f);
                c.drawPath(path, ink);
                break;
            case SETTINGS: {
                // A cog: teeth round a ring, with a hole.
                int teeth = 8;
                float outer = s, inner = s * 0.72f;
                for (int i = 0; i < teeth * 2; i++) {
                    double a0 = Math.PI * 2 * i / (teeth * 2) - Math.PI / (teeth * 2);
                    double a1 = Math.PI * 2 * (i + 1) / (teeth * 2) - Math.PI / (teeth * 2);
                    float rr = (i % 2 == 0) ? outer : inner;
                    float x0 = cx + (float) Math.cos(a0) * rr, y0 = cy + (float) Math.sin(a0) * rr;
                    float x1 = cx + (float) Math.cos(a1) * rr, y1 = cy + (float) Math.sin(a1) * rr;
                    if (i == 0) path.moveTo(x0, y0); else path.lineTo(x0, y0);
                    path.lineTo(x1, y1);
                }
                path.close();
                ink.setStyle(Paint.Style.FILL);
                c.drawPath(path, ink);
                disc.setColor(isPressed() ? Color.rgb(78, 88, 110) : Color.rgb(38, 43, 56));
                c.drawCircle(cx, cy, s * 0.32f, disc);
                break;
            }
            case MENU:
                // The Deck's ⋯.
                ink.setStyle(Paint.Style.FILL);
                for (int i = -1; i <= 1; i++) c.drawCircle(cx + i * s * 0.75f, cy, s * 0.17f, ink);
                break;
            case KEYBOARD:
            case KEYBOARD_HIDE: {
                boolean hide = icon == Icon.KEYBOARD_HIDE;
                float top = hide ? cy - s * 0.85f : cy - s * 0.6f;
                box.set(cx - s, top, cx + s, top + s * 1.2f);
                ink.setStrokeWidth(r * 0.07f);
                c.drawRoundRect(box, s * 0.15f, s * 0.15f, ink);
                ink.setStyle(Paint.Style.FILL);
                float key = s * 0.09f;
                for (int row = 0; row < 2; row++) {
                    for (int col = 0; col < 5; col++) {
                        c.drawCircle(cx - s * 0.64f + col * s * 0.32f, top + s * 0.3f + row * s * 0.3f, key, ink);
                    }
                }
                ink.setStyle(Paint.Style.STROKE);
                ink.setStrokeWidth(r * 0.07f);
                c.drawLine(cx - s * 0.45f, top + s * 0.92f, cx + s * 0.45f, top + s * 0.92f, ink);
                if (hide) {
                    // And a chevron under it: put it away.
                    ink.setStrokeWidth(r * 0.09f);
                    path.moveTo(cx - s * 0.35f, cy + s * 0.6f);
                    path.lineTo(cx, cy + s * 0.9f);
                    path.lineTo(cx + s * 0.35f, cy + s * 0.6f);
                    c.drawPath(path, ink);
                }
                break;
            }
        }
    }
}
