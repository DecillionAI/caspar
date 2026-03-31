package actions_machine

import "testing"

func TestEntityRuntimeFileName(t *testing.T) {
	tests := map[string]string{
		"wasm":       "module.wasm",
		"javascript": "module.js",
		"elpify":     "module.masm",
		"elpian":     "module.elpian.json",
	}
	for runtime, want := range tests {
		if got := entityRuntimeFileName(runtime); got != want {
			t.Fatalf("runtime %s -> %s, want %s", runtime, got, want)
		}
	}
}

func TestIsSupportedEntityType(t *testing.T) {
	supported := []string{"docker", "wasm", "javascript", "elpify", "elpian", " ElPiAn "}
	for _, runtime := range supported {
		if !isSupportedEntityType(runtime) {
			t.Fatalf("expected runtime %q to be supported", runtime)
		}
	}

	unsupported := []string{"", "python", "node", "masm"}
	for _, runtime := range unsupported {
		if isSupportedEntityType(runtime) {
			t.Fatalf("expected runtime %q to be unsupported", runtime)
		}
	}
}

func TestElpianFlowConfigSequence(t *testing.T) {
	// This mirrors the deploy/run configuration path for an elpian entity:
	// create machine -> create program -> create entity -> deploy -> run.
	runtime := "elpian"
	if !isSupportedEntityType(runtime) {
		t.Fatal("elpian runtime must be supported in machine/program/entity flows")
	}
	artifact := entityRuntimeFileName(runtime)
	if artifact != "module.elpian.json" {
		t.Fatalf("elpian artifact mismatch: got %s", artifact)
	}
}
