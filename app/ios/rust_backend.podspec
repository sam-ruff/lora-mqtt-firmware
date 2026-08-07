Pod::Spec.new do |s|
  s.name             = 'rust_backend'
  s.version          = '1.0.0'
  s.summary          = 'Walkie Textie hub configurator Rust engine.'
  s.description      = 'Static library exposing the hub provisioning client to the app over flutter_rust_bridge.'
  s.homepage         = 'https://github.com/sam-ruff/walkie-textie-hub-firmware'
  s.license          = { :type => 'MIT' }
  s.author           = { 'Walkie Textie' => 'noreply@meshcore.dev' }
  s.source           = { :path => '.' }

  # Built by tool/build_ios_libs.sh before pod install.
  s.vendored_frameworks = 'Frameworks/rust_backend.xcframework'
  s.requires_arc     = true

  # Dart reaches the engine only via dlsym at runtime, so nothing references
  # it at link time; the force-load lives in ios/Flutter/*.xcconfig.
end
