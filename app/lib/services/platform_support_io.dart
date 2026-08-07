import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';

import '../src/rust/frb_generated.dart';

/// Initialise the Rust engine with the right loader per platform: the engine
/// is statically linked into the app binary on Apple platforms (see
/// ios/Flutter/*.xcconfig), so it loads from the process rather than a
/// separate dynamic library.
Future<void> initRustLib() async {
  if (Platform.isIOS || Platform.isMacOS) {
    await RustLib.init(externalLibrary: ExternalLibrary.process(iKnowHowToUseIt: true));
  } else {
    await RustLib.init();
  }
}
