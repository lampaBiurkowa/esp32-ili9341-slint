import asyncio
import websockets
import logging

logging.basicConfig(level=logging.DEBUG)

async def handler(ws):
    async for msg in ws:
        print("RX:", msg)
        await ws.send("ack")

async def main():
    async with websockets.serve(handler, "0.0.0.0", 8765):
        print("WebSocket server running on ws://0.0.0.0:8765")
        await asyncio.Future()  # run forever

asyncio.run(main())
