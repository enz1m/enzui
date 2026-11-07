fn rgb_u8(c: Color) -> (u8, u8, u8) {
    (
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    )
}
