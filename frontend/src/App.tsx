import { useState } from "react";
import "./App.css";
import { Button } from "./components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

function App() {
  const [count, setCount] = useState(0);

  return (
    <div className="w-dvw h-dvh flex flex-col justify-center items-center">
      <Card className="w-80 h-56">
        <CardHeader>
          <CardTitle>Card Title</CardTitle>
          <CardDescription>Card Description</CardDescription>
        </CardHeader>

        <CardContent className="flex-1">
          <p>Card Content</p>
        </CardContent>

        <CardFooter >
          <Button onClick={() => setCount(count + 1)}>
            Count {count}
          </Button>
        </CardFooter>
      </Card>
    </div>
  );
}

export default App;
