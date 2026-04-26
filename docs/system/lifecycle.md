# Phase4 Lifecycle Diagrams

## Startup Lifecycle

```mermaid
%%{init: {
	"theme": "base",
	"themeVariables": {
		"background": "#ffffff",
		"fontFamily": "monospace",
		"fontSize": "16px",
		"lineColor": "#000",
		"primaryBorderColor": "#000",
		"primaryColor": "#fff",
		"primaryTextColor": "#000",
		"secondaryColor": "#aaa",
		"tertiaryColor": "#ccc"
	},
	"flowchart": {
		"curve": "basis",
		"htmlLabels": true
	}
}}%%
flowchart TD
		A[main] --> B[Parse CLI args]
		B --> C{--list?}
		C -->|yes| D[List input devices and exit]
		C -->|no| E[Build AppConfig from args]
		E --> F{Calibration mode?}
		F -->|yes| G[Use synthetic specs 44.1kHz, 2ch]
		F -->|no| H[Resolve input device and native specs]
		G --> I[Create record and analyse ring buffers]
		H --> I
		I --> J[Create watch channels: RawPayload and DisplayPayload JSON]
		J --> K{Input source}
		K -->|calibration| M[Spawn generator thread]
		K -->|hardware| L[Start CPAL stream callback]
		L --> N[Spawn recorder thread]
		M --> N
		N --> O[Spawn analyser thread]
		O --> P[Spawn mapper thread]
		P --> Q[Spawn WebSocket server thread]
		Q --> R[Run controller loop until shutdown]
		R --> S[Shutdown: drop input, signal keep_running=false, join workers with timeouts]
```

## Runtime Data Flow

```mermaid
%%{init: {
	"theme": "base",
	"themeVariables": {
		"background": "#ffffff",
		"fontFamily": "monospace",
		"fontSize": "16px",
		"lineColor": "#000",
		"primaryBorderColor": "#000",
		"primaryColor": "#fff",
		"primaryTextColor": "#000",
		"secondaryColor": "#aaa",
		"tertiaryColor": "#ccc"
	},
	"flowchart": {
		"curve": "basis",
		"htmlLabels": true
	}
}}%%
flowchart LR
		subgraph Source
			A1[CPAL input callback]:::source
			A2[Generator thread]:::source
		end

		A1 -->|f32 interleaved frames| B1[(Record ring buffer)]
		A1 -->|f32 interleaved frames| B2[(Analyse ring buffer)]
		A2 -->|synthetic f32 frames| B1
		A2 -->|synthetic f32 frames| B2

		B1 --> C1[Recorder thread]
		C1 --> D1[WAV file in recordings]

		B2 --> C2[Analyser thread]
		C2 -->|watch send_replace RawPayload| E[(RawPayload watch)]
		E --> F[Mapper thread]
		F -->|map bins plus JSON serialise| G[(Display JSON watch)]
		G --> H[WebSocket server]
		H --> I[Clients: browser or native]

		J[Controller thread] -->|toggle atomics| C1
		J -->|toggle atomics| C2
		J -->|toggle atomics| F
		J -->|toggle atomics| H

```

## Controller State Lifecycle

```mermaid
%%{init: {
	"theme": "base",
	"themeVariables": {
		"background": "#ffffff",
		"fontFamily": "monospace",
		"fontSize": "16px",
		"lineColor": "#000",
		"primaryBorderColor": "#000",
		"primaryColor": "#fff",
		"primaryTextColor": "#000",
		"secondaryColor": "#aaa",
		"tertiaryColor": "#ccc"
	}
}}%%
stateDiagram-v2
		[*] --> Running

		state Running {
			[*] --> Flags
			Flags --> Flags: B key toggles is_broadcasting_websocket
			Flags --> Flags: R key toggles is_recording
			Flags --> Flags: A key toggles is_analysing
			Flags --> ExitRequested: Ctrl+C sets keep_running=false
		}

		ExitRequested --> Shutdown
		Shutdown --> [*]
```

## Shutdown Ordering and Timeouts

```mermaid
sequenceDiagram
		participant Main as App.shutdown
		participant In as Input stream/device
		participant Gen as Generator
		participant An as Analyser
		participant Map as Mapper
		participant Srv as Server
		participant Rec as Recorder

		Main->>In: Drop input handle
		Main->>Main: keep_running=false

		Main->>Gen: join with 250ms timeout
		Main->>An: join with 1000ms timeout
		Main->>Map: join with 1000ms timeout
		Main->>Srv: join with 1500ms timeout
		Main->>Rec: join with RECORD_BUFFER_MS + 2000ms timeout

		note over Main,Srv: Server notifies clients, sends close frames, bounded task join
		note over Main,Rec: Recorder drains remaining ring buffer and finalises WAV
```
