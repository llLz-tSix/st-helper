"""Render a demo GIF of the ST Helper overlay: question being recognized, answer streaming."""
import sys
from PIL import Image, ImageDraw, ImageFont, ImageFilter

OUT = sys.argv[1]
W, H = 760, 428
S = 2  # supersampling
F = "C:/Windows/Fonts/"
def font(name, size): return ImageFont.truetype(F + name, size * S)
f_status = font("segoeui.ttf", 12)
f_q = font("segoeui.ttf", 12)
f_a = font("segoeui.ttf", 16)
f_btn = font("seguisym.ttf", 12)
f_label = font("segoeui.ttf", 13)
f_badge = font("segoeuib.ttf", 11)
f_title = font("segoeuib.ttf", 13)

STATES = {
    "listening": ((62, 207, 110), "Слушаю"),
    "transcribing": ((74, 163, 255), "Распознаю…"),
    "generating": ((240, 180, 41), "Генерирую ответ…"),
}

def backdrop():
    """A generic dark video-call window behind the overlay."""
    img = Image.new("RGB", (W * S, H * S), (12, 13, 17))
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, W * S, 34 * S], fill=(22, 23, 29))
    d.text((16 * S, 9 * S), "Собеседование · Видеозвонок", font=f_title, fill=(200, 202, 210))
    for i, (cx, name, col) in enumerate([(190, "Интервьюер", (88, 101, 242)), (420, "Вы", (60, 160, 120))]):
        x0, y0 = (cx - 110) * S, 120 * S
        d.rounded_rectangle([x0, y0, x0 + 220 * S, y0 + 150 * S], 12 * S, fill=(26, 28, 35))
        d.ellipse([(cx - 34) * S, 150 * S, (cx + 34) * S, 218 * S], fill=col)
        d.text((cx * S, 240 * S), name, font=f_label, fill=(190, 192, 200), anchor="mm")
        if i == 0:
            d.rounded_rectangle([x0 - 3 * S, y0 - 3 * S, x0 + 223 * S, y0 + 153 * S], 14 * S, outline=(62, 207, 110), width=2 * S)
    d.rounded_rectangle([250 * S, 350 * S, 360 * S, 390 * S], 10 * S, fill=(40, 42, 50))
    d.rounded_rectangle([372 * S, 350 * S, 452 * S, 390 * S], 10 * S, fill=(200, 60, 66))
    return img

BG = backdrop()
OX, OY, OW, OH = 300, 60, 440, 250  # overlay rect

def wrap(d, text, fnt, width):
    lines = []
    for para in text.split("\n"):
        line = ""
        for word in para.split(" "):
            t = (line + " " + word).strip()
            if d.textlength(t, font=fnt) <= width * S:
                line = t
            else:
                lines.append(line)
                line = word
        lines.append(line)
    return lines

def frame(state, question, answer, fade_tail=0, hint=False):
    img = BG.copy()
    # translucent overlay panel
    layer = Image.new("RGBA", img.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    box = [OX * S, OY * S, (OX + OW) * S, (OY + OH) * S]
    region = img.crop(box).filter(ImageFilter.GaussianBlur(0))
    d.rounded_rectangle(box, 10 * S, fill=(16, 17, 22, int(255 * 0.78)), outline=(255, 255, 255, 22), width=S)
    d.rectangle([(OX + 1) * S, (OY + 1) * S, (OX + OW - 1) * S, (OY + 26) * S], fill=(255, 255, 255, 10))
    col, label = STATES[state]
    d.ellipse([(OX + 10) * S, (OY + 9) * S, (OX + 18) * S, (OY + 17) * S], fill=col + (255,))
    d.text(((OX + 26) * S, (OY + 13) * S), label, font=f_status, fill=(185, 187, 198, 255), anchor="lm")
    for i, ch in enumerate(["❚❚", "⚙", "✕"]):
        d.text(((OX + OW - 62 + i * 22) * S, (OY + 13) * S), ch, font=f_btn, fill=(154, 157, 171, 255), anchor="mm")
    y = OY + 34
    if question:
        for ln in wrap(d, question, f_q, OW - 24)[:3]:
            d.text(((OX + 12) * S, y * S), ln, font=f_q, fill=(148, 151, 163, 255))
            y += 17
        y += 4
    if hint:
        d.text(((OX + 12) * S, y * S), "Ожидаю вопрос…", font=f_q, fill=(111, 114, 128, 255))
    elif answer:
        lines = wrap(d, answer, f_a, OW - 24)
        total = sum(len(l) for l in lines)
        shown = 0
        for ln in lines:
            # the newest `fade_tail` characters are still fading in
            x = OX + 12
            for ch in ln:
                age = total - shown  # 1 = newest
                a = 255 if age > fade_tail else 255 - int(175 * (fade_tail - age + 1) / fade_tail)
                d.text((x * S, y * S), ch, font=f_a, fill=(242, 242, 245, a))
                x += d.textlength(ch, font=f_a) / S
                shown += 1
            y += 23
    # grip
    gx, gy = OX + OW - 2, OY + OH - 2
    d.polygon([(gx * S, (gy - 12) * S), (gx * S, gy * S), ((gx - 12) * S, gy * S)], fill=(255, 255, 255, 60))
    # badge
    bx, by = OX + OW - 196, OY + OH + 12
    d.rounded_rectangle([bx * S, by * S, (bx + 196) * S, (by + 24) * S], 12 * S, fill=(47, 111, 214, 235))
    d.text(((bx + 98) * S, (by + 12) * S), "скрыто от записи и демонстрации", font=f_badge, fill=(255, 255, 255, 255), anchor="mm")
    img = Image.alpha_composite(img.convert("RGBA"), layer).convert("RGB")
    return img.resize((W, H), Image.LANCZOS)

Q1 = "Объясните, пожалуйста, чем процесс отличается от потока в операционной системе?"
A1 = ("Процесс — это изолированный экземпляр программы со своим адресным пространством и ресурсами. "
      "Поток — лёгкая единица выполнения внутри процесса: потоки делят общую память, но у каждого свой стек. "
      "Поэтому процессы дают изоляцию, а потоки — дешёвый параллелизм.")
Q2 = "Расскажите о конфликте в команде и как вы его решили?"
A2 = ("- Сначала выясняю причину разногласий и выслушиваю обе стороны.\n"
      "- Свожу спор к фактам: требования, сроки, риски.\n"
      "- Предлагаю решение и фиксирую договорённости, чтобы не повторялось.")

frames, durs = [], []
def add(img, ms): frames.append(img); durs.append(ms)

def scene(q, a):
    add(frame("listening", "", "", hint=True), 900)
    for i in range(0, len(q) + 1, 3):  # recognized text appears
        add(frame("transcribing", q[:i], "", hint=True), 45)
    add(frame("transcribing", q, "", hint=True), 400)
    add(frame("generating", q, ""), 300)
    words = a.split(" ")
    for i in range(1, len(words) + 1):
        add(frame("generating", q, " ".join(words[:i]), fade_tail=8), 70)
    add(frame("listening", q, a), 2600)

scene(Q1, A1)
scene(Q2, A2)
# One shared palette that is guaranteed to contain the small status-dot colors.
sample = Image.new("RGB", (W, H * 2 + 40))
sample.paste(frames[len(frames) // 4], (0, 0))
sample.paste(frames[-1], (0, H))
sd = ImageDraw.Draw(sample)
for i, (col, _) in enumerate(STATES.values()):
    sd.rectangle([i * 120, H * 2, i * 120 + 119, H * 2 + 39], fill=col)
pal = sample.quantize(colors=255, method=Image.Quantize.MEDIANCUT)
frames = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f in frames]
frames[0].save(OUT, save_all=True, append_images=frames[1:], duration=durs, loop=0, optimize=True, disposal=1)
print(len(frames), "frames")
