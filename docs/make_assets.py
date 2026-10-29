"""Builds README images in docs/img.

usage: python make_assets.py <overlay.png> <settings.png>
Both PNGs are headless-Edge renders (2x) of src/index.html and src/settings.html
with a stubbed Tauri API; the real window itself can't be captured.
"""
import os
import sys
from PIL import Image, ImageDraw, ImageFilter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import make_demo_gif as m  # noqa: E402

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "img")
os.makedirs(OUT, exist_ok=True)
overlay_png, settings_png = sys.argv[1], sys.argv[2]
KEY = [c for c, _ in m.STATES.values()] + [(229, 72, 77), (47, 111, 214)]
f = m.font


def gradient(w, h, top, bottom):
    img = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(img)
    for y in range(h):
        t = y / (h - 1)
        d.line([(0, y), (w, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bottom)))
    return img


def pill(d, xy, text, fnt, fill, color=(255, 255, 255)):
    x, y = xy
    w = d.textlength(text, font=fnt) + 28
    d.rounded_rectangle([x, y, x + w, y + 30], 15, fill=fill)
    d.text((x + w / 2, y + 15), text, font=fnt, fill=color, anchor="mm")
    return x + w


# --- banner (also fits GitHub social preview 1280x640) ---------------------
def banner():
    W, H = 1280, 640
    img = gradient(W, H, (20, 24, 38), (8, 9, 13))
    d = ImageDraw.Draw(img)
    # glow
    glow = Image.new("RGB", (W, H), (0, 0, 0))
    ImageDraw.Draw(glow).ellipse([700, 60, 1300, 600], fill=(30, 60, 130))
    img = Image.blend(img, glow.filter(ImageFilter.GaussianBlur(120)), 0.35)
    d = ImageDraw.Draw(img)
    # left: title
    icon = Image.open(os.path.join(os.path.dirname(OUT), "..", "src-tauri", "icons", "128x128@2x.png")).convert("RGBA").resize((88, 88), Image.LANCZOS)
    img.paste(icon, (72, 120), icon)
    d.text((176, 126), "ST Helper", font=f("segoeuib.ttf", 30), fill=(245, 246, 250))  # 60px (font() doubles)
    d.text((72, 236), "Подсказки на собеседовании", font=f("segoeuib.ttf", 17), fill=(225, 228, 236))
    d.text((72, 276), "в реальном времени", font=f("segoeuib.ttf", 17), fill=(225, 228, 236))
    small = f("segoeui.ttf", 10)
    d.text((72, 338), "Слушает собеседника, распознаёт вопрос и стримит", font=small, fill=(150, 155, 170))
    d.text((72, 366), "короткий ответ. Окно не видно в демонстрации экрана.", font=small, fill=(150, 155, 170))
    chip = f("segoeuib.ttf", 8)
    x = 72
    for t, c in [("Windows 11", (40, 44, 56)), ("Tauri 2 · Rust", (40, 44, 56)), ("Groq · бесплатно", (40, 44, 56)), ("7 МБ", (40, 44, 56))]:
        x = pill(d, (x, 430), t, chip, c, (210, 214, 224)) + 10
    # right: real overlay render on a call backdrop
    call = m.BG.resize((620, 349), Image.LANCZOS).crop((0, 0, 560, 349)).filter(ImageFilter.GaussianBlur(1.2))
    frame = Image.new("RGB", (580, 369), (35, 38, 48))
    frame.paste(call, (10, 10))
    img.paste(frame, (640, 130))
    ov = Image.open(overlay_png).convert("RGBA")
    ov = ov.resize((int(ov.width * 0.46), int(ov.height * 0.46)), Image.LANCZOS)
    shadow = Image.new("RGBA", (ov.width + 60, ov.height + 60), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle([30, 34, ov.width + 30, ov.height + 34], 12, fill=(0, 0, 0, 150))
    shadow = shadow.filter(ImageFilter.GaussianBlur(14))
    base = img.convert("RGBA")
    base.alpha_composite(shadow, (712 - 30, 200 - 30))
    base.alpha_composite(ov, (712, 200))
    img = base.convert("RGB")
    d = ImageDraw.Draw(img)
    pill(d, (972, 520), "скрыто от записи экрана", chip, (47, 111, 214))
    img.save(os.path.join(OUT, "banner.png"), optimize=True)


# --- privacy GIF: your screen vs what the interviewer sees ----------------
def privacy():
    q = "Как бы вы спроектировали идемпотентную обработку платежей?"
    a = ("Каждый запрос несёт ключ идемпотентности, я храню его в PostgreSQL с уникальным индексом. "
         "Повтор с тем же ключом возвращает сохранённый результат и не проводит платёж второй раз.")
    frames, durs = m.scene_frames([(q, a)])
    PW, PH = 620, 349
    W, H = 2 * PW + 60, PH + 90
    bg = gradient(W, H, (16, 18, 26), (9, 10, 14))
    hd = f("segoeuib.ttf", 8)
    lb = f("segoeui.ttf", 7)
    right = m.BG.resize((PW, PH), Image.LANCZOS)
    rd = ImageDraw.Draw(right)
    rd.rounded_rectangle([PW - 150, 12, PW - 12, 40], 14, fill=(229, 72, 77))
    rd.text((PW - 81, 26), "● Демонстрация", font=lb, fill=(255, 255, 255), anchor="mm")
    out = []
    for fr in frames:
        img = bg.copy()
        d = ImageDraw.Draw(img)
        d.text((20, 26), "Что видите вы", font=hd, fill=(235, 237, 244))
        d.text((40 + PW, 26), "Что видит собеседник в демонстрации экрана", font=hd, fill=(235, 237, 244))
        img.paste(fr.resize((PW, PH), Image.LANCZOS), (20, 64))
        img.paste(right, (40 + PW, 64))
        out.append(img)
    m.save_gif(os.path.join(OUT, "privacy.gif"), out, durs, KEY)


# --- settings screenshot: drop the empty area above the footer -------------
def settings():
    s = Image.open(settings_png).convert("RGB")
    footer_h = 170
    content = s.crop((0, 0, s.width, 2240))
    footer = s.crop((0, s.height - footer_h, s.width, s.height))
    out = Image.new("RGB", (s.width, content.height + footer_h), (21, 22, 27))
    out.paste(content, (0, 0))
    out.paste(footer, (0, content.height))
    out = out.resize((out.width // 2, out.height // 2), Image.LANCZOS)
    out.save(os.path.join(OUT, "settings.png"), optimize=True)


def overlay():
    ov = Image.open(overlay_png).convert("RGBA")
    call = m.BG.resize((ov.width + 80, ov.height + 80), Image.LANCZOS).convert("RGBA")
    call.alpha_composite(ov, (40, 40))
    call.convert("RGB").resize((call.width // 2, call.height // 2), Image.LANCZOS).save(os.path.join(OUT, "overlay.png"), optimize=True)


if __name__ == "__main__":
    banner()
    settings()
    overlay()
    privacy()
    frames, durs = m.scene_frames([(m.Q1, m.A1), (m.Q2, m.A2)])
    m.save_gif(os.path.join(OUT, "demo.gif"), frames, durs, KEY)
