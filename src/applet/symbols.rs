use cosmic::widget;

macro_rules! bundled {
    ($($name:ident => $file:literal,)*) => {
        $(
            pub fn $name() -> widget::icon::Handle {
                widget::icon::from_svg_bytes(
                    include_bytes!(concat!("../../resources/icons/", $file)).as_slice(),
                )
                .symbolic(true)
            }
        )*
    };
}

bundled! {
    appearance => "appearance-symbolic.svg",
    bug => "bug-symbolic.svg",
    code => "code-symbolic.svg",
    droplet => "droplet-symbolic.svg",
    grid => "grid-symbolic.svg",
    grip => "grip-symbolic.svg",
    link => "link-symbolic.svg",
    person => "person-symbolic.svg",
    settings => "settings-symbolic.svg",
}
