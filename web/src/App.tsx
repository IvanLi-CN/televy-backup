import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export default function App() {
	const [pingResult, setPingResult] = useState<string>("");

	useEffect(() => {
		void invoke<string>("ping", { value: "TelevyBackup" }).then(setPingResult);
	}, []);

	return (
		<main className="container">
			<h1>TelevyBackup</h1>
			<p>Backend says: {pingResult}</p>
		</main>
	);
}
