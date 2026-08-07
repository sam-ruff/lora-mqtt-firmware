import '../src/rust/frb_generated.dart';

Future<void> initRustLib() async {
  await RustLib.init();
}
