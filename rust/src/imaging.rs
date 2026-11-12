use anyhow::{anyhow, ensure, Context as _, Result};
use image::{DynamicImage, ImageFormat, RgbaImage};
use rquickjs::{Context, Function, Runtime};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    io::Cursor,
    rc::Rc,
    time::{Duration, Instant},
};

const MAX_PIXELS: u64 = 100_000_000;
fn dimensions(w: u32, h: u32) -> Result<()> {
    ensure!(
        w > 0 && h > 0 && u64::from(w) * u64::from(h) <= MAX_PIXELS,
        "Invalid or oversized image dimensions"
    );
    Ok(())
}
pub fn decode(data: &[u8]) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    let image = reader.decode()?;
    dimensions(image.width(), image.height())?;
    Ok(image)
}

struct Images(Vec<RgbaImage>);
impl Images {
    fn insert(&mut self, image: RgbaImage) -> Result<Value> {
        ensure!(
            self.0
                .iter()
                .map(|i| u64::from(i.width()) * u64::from(i.height()))
                .sum::<u64>()
                + u64::from(image.width()) * u64::from(image.height())
                <= MAX_PIXELS,
            "Image script memory limit exceeded"
        );
        let key = self.0.len();
        self.0.push(image);
        Ok(json!(key))
    }
    fn handle(&mut self, data: &Value) -> Result<Value> {
        ensure!(
            data["method"] == "image",
            "Unsupported image script host method"
        );
        let n = |name: &str| -> Result<u32> {
            data[name]
                .as_u64()
                .and_then(|v| v.try_into().ok())
                .with_context(|| format!("Invalid {name}"))
        };
        let operation = data["function"]
            .as_str()
            .context("Missing image operation")?;
        if operation == "emptyImage" {
            let (w, h) = (n("width")?, n("height")?);
            dimensions(w, h)?;
            return self.insert(RgbaImage::new(w, h));
        }
        let key = n("key")? as usize;
        let source = self.0.get(key).context("Unknown image handle")?;
        match operation {
            "getWidth" => Ok(json!(source.width())),
            "getHeight" => Ok(json!(source.height())),
            "copyAndRotate90" => {
                let copy = image::imageops::rotate90(source);
                self.insert(copy)
            }
            "copyRange" => {
                let (x, y, w, h) = (n("x")?, n("y")?, n("width")?, n("height")?);
                bounds(source, x, y, w, h)?;
                let copy = image::imageops::crop_imm(source, x, y, w, h).to_image();
                self.insert(copy)
            }
            "fillImageAt" | "fillImageRangeAt" => {
                let other = self
                    .0
                    .get(n("image")? as usize)
                    .context("Unknown source image handle")?;
                let (x, y) = (n("x")?, n("y")?);
                let (sx, sy, w, h) = if operation == "fillImageAt" {
                    (0, 0, other.width(), other.height())
                } else {
                    (n("srcX")?, n("srcY")?, n("width")?, n("height")?)
                };
                bounds(source, x, y, w, h)?;
                bounds(other, sx, sy, w, h)?;
                let copy = image::imageops::crop_imm(other, sx, sy, w, h).to_image();
                image::imageops::replace(&mut self.0[key], &copy, i64::from(x), i64::from(y));
                Ok(Value::Null)
            }
            _ => Err(anyhow!("Unknown image operation: {operation}")),
        }
    }
}
fn bounds(image: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> Result<()> {
    dimensions(w, h)?;
    ensure!(
        u64::from(x) + u64::from(w) <= u64::from(image.width())
            && u64::from(y) + u64::from(h) <= u64::from(image.height()),
        "Image rectangle out of bounds"
    );
    Ok(())
}

pub fn modify(data: Vec<u8>, script: String) -> Result<Vec<u8>> {
    let images = Rc::new(RefCell::new(Images(vec![decode(&data)?.to_rgba8()])));
    let runtime = Runtime::new()?;
    runtime.set_memory_limit(64 * 1024 * 1024);
    runtime.set_max_stack_size(2 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(30);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime)?;
    let key = context.with(|ctx| -> Result<usize> {
        let state = images.clone();
        let callback = Function::new(ctx.clone(), move |message: String| -> String {
            let result = (|| {
                let data = crate::script::unpack(&serde_json::from_str(&message)?)?;
                state.borrow_mut().handle(&data)
            })();
            match result {
                Ok(v) => json!({"value":crate::script::pack(&v)}).to_string(),
                Err(e) => json!({"error":format!("{e:#}")}).to_string(),
            }
        })?;
        ctx.globals().set("__nativeHost", callback)?;
        ctx.eval::<(), _>(include_str!("script_bridge.js"))?;
        ctx.eval::<(), _>(include_str!("../../assets/init.js"))?;
        ctx.eval::<(), _>(script)
            .map_err(|e| crate::script::js_error(&ctx, e))?;
        ctx.eval::<usize, _>("modifyImage(new Image(0)).key")
            .map_err(|e| crate::script::js_error(&ctx, e))
    })?;
    let images = images.borrow();
    let image = images
        .0
        .get(key)
        .context("Image script returned an invalid image")?;
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, ImageFormat::Png)?;
    Ok(output.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_tiles_rotation_and_bounds() -> Result<()> {
        let mut pixels = RgbaImage::new(2, 1);
        pixels.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        pixels.put_pixel(1, 0, image::Rgba([0, 0, 255, 128]));
        let mut input = Cursor::new(Vec::new());
        pixels.write_to(&mut input, ImageFormat::Png)?;
        let result = modify(
            input.get_ref().clone(),
            "function modifyImage(i) { return i.copyAndRotate90(); }".into(),
        )?;
        let output = decode(&result)?.to_rgba8();
        assert_eq!(output.dimensions(), (1, 2));
        assert_eq!(output.get_pixel(0, 1).0, [0, 0, 255, 128]);
        assert!(modify(
            input.into_inner(),
            "function modifyImage(i) { return i.copyRange(-1,0,2,1); }".into()
        )
        .is_err());
        Ok(())
    }
}
