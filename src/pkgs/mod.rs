pub mod archlinux;
pub mod debian;

#[derive(Debug, PartialEq)]
pub struct Pkg {
    pub name: String,
    pub version: String,
}
