/*
    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        https://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

fn main() {
    #[cfg(feature = "cpp-ffi")]
    {
        cxx_build::bridge("src/ffi/mod.rs")
            .flag_if_supported("-std=c++17")
            .compile("adaptive_engine_cxx");
    }

    println!("cargo:rerun-if-changed=src/ffi/mod.rs");
    println!("cargo:rerun-if-changed=src/ffi/engine.rs");
    println!("cargo:rerun-if-changed=src/ffi/callbacks.rs");
}
