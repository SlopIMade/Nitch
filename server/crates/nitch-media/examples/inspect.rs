use gstreamer as gst;
use gst::prelude::*;
fn main() {
    gst::init().unwrap();
    gstfallbackswitch::plugin_register_static().unwrap();
    gsthlssink3::plugin_register_static().unwrap();
    gstfmp4::plugin_register_static().unwrap();
    for name in std::env::args().skip(1) {
        let e = gst::ElementFactory::make(&name).build().unwrap();
        println!("== {name}");
        for p in e.list_properties() {
            let v = e.property_value(p.name()).serialize().map(|s| s.to_string()).unwrap_or_default();
            println!("  {:<28} {:<14} {}", p.name(), p.value_type().name(), v.chars().take(140).collect::<String>());
        }
        for t in e.pad_template_list() { println!("  pad {} {:?} {:?}", t.name_template(), t.direction(), t.presence()); }
    }
}
