import { BrowserRouter, Routes, Route, Navigate } from "react-router-dom";
import Remote from "./pages/Remote";
import Session from "./pages/Session";
import Settings from "./pages/Settings";
import "./App.css";

function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Remote />} />
        <Route path="/session" element={<Session />} />
        <Route path="/settings" element={<Settings />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
