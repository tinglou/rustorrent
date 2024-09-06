---
created: 2024-09-06T16:51:34 (UTC +08:00)
tags: []
source: http://www.bittorrent.org/beps/bep_0029.html
author: Arvid Norberg <arvid@bittorrent.com>
---


# uTorrent Transport Protocol
---

## credits

The uTorrent transport protocol was designed by Ludvig Strigeus, Greg Hazel, Stanislav Shalunov, Arvid Norberg and Bram Cohen.

## rationale

The motivation for uTP is for BitTorrent clients to not disrupt internet connections, while still utilizing the unused bandwidth fully.

The problem is that DSL and cable modems typically have a send buffer disproportional to their max send rate, which can hold several seconds worth of packets. BitTorrent traffic is typically background transfers, and should have lower priority than checking email, phone calls and browsing the web, but when using regular TCP connections BitTorrent quickly fills up the send buffer, adding multiple seconds delay to all interactive traffic.

The fact that BitTorrent uses multiple TCP connections gives it an unfair advantage when competing with other services for bandwidth, which exaggerates the effect of BitTorrent filling the upload pipe. The reason for this is because TCP distributes the available bandwidth evenly across connections, and the more connections one application uses, the larger share of the bandwidth it gets.

The traditional solution to this problem is to cap the upload rate of the BitTorrent client to 80% of the up-link capacity. 80% leaves some head room for interactive traffic.

The main drawbacks with this solution are:

1.  The user needs to configure his/her BitTorrent client, it won't work out-of-the-box.
2.  The user needs to know his/her internet connection's upload capacity. This capacity may change, especially on laptops that may connect to a large number of different networks.
3.  The headroom of 20% is arbitrary and wastes bandwidth. Whenever there is no interactive traffic competing with BitTorrent, the extra 20% are wasted. Whenever there is competing interactive traffic, it cannot use more than 20% of the capacity.

uTP solves this problem by using the modem queue size as a controller for its send rate. When the queue grows too large, it throttles back.

This lets it utilize the full upload capacity when there is no competition for it, and it lets it throttle back to virtually nothing when there is a lot of interactive traffic.

## overview

This document assumes some knowledge of how TCP and window based congestion control works.

uTP is a transport protocol layered on top of UDP. As such, it must (and has the ability to) implement its own congestion control.

The main difference compared to TCP is the delay based congestion control. See the [congestion control](http://www.bittorrent.org/beps/bep_0029.html#congestion-control) section.

Like TCP, uTP uses window based congestion control. Each socket has a max\_window which determines the maximum number of bytes the socket may have _in-flight_ at any given time. Any packet that has been sent, but not yet acked, is considered to be in-flight.

The number of bytes in-flight is cur\_window.

A socket may only send a packet if cur\_window + packet\_size is less than or equal to min(max\_window, wnd\_size). The packet size may vary, see the [packet sizes](http://www.bittorrent.org/beps/bep_0029.html#packet-sizes) section.

wnd\_size is the advertised window from the other end. It sets an upper limit on the number of packets in-flight.

An implementation MAY violate the above rule if the max\_window is smaller than the packet size, and it paces the packets so that the average cur\_window is less than or equal to max\_window.

Each socket keeps a state for the last delay measurement from the other endpoint (reply\_micro). Whenever a packet is received, this state is updated by subtracting timestamp\_microseconds from the hosts current time, in microseconds (see [header format](http://www.bittorrent.org/beps/bep_0029.html#header-format)).

Every time a packet is sent, the sockets reply\_micro value is put in the timestamp\_difference\_microseconds field of the packet header.

Unlike TCP, sequence numbers and ACKs in uTP refers to packets, not bytes. This means uTP cannot _repackage_ data when resending it.

Each socket keeps a state of the next sequence number to use when sending a packet, seq\_nr. It also keeps a state of the sequence number that was last received, ack\_nr. The oldest unacked packet is seq\_nr - cur\_window.

## connection setup

Here is a diagram illustrating the exchanges and states to initiate a connection. The c.\* refers to a state in the socket itself, pkt.\* refers to a field in the packet header.

```
initiating endpoint                           accepting endpoint

          | c.state = CS_SYN_SENT                         |
          | c.seq_nr = 1                                  |
          | c.conn_id_recv = rand()                       |
          | c.conn_id_send = c.conn_id_recv + 1           |
          |                                               |
          |                                               |
          | ST_SYN                                        |
          |   seq_nr=c.seq_nr++                           |
          |   ack_nr=*                                    |
          |   conn_id=c.rcv_conn_id                       |
          | >-------------------------------------------> |
          |             c.receive_conn_id = pkt.conn_id+1 |
          |             c.send_conn_id = pkt.conn_id      |
          |             c.seq_nr = rand()                 |
          |             c.ack_nr = pkt.seq_nr             |
          |             c.state = CS_SYN_RECV             |
          |                                               |
          |                                               |
          |                                               |
          |                                               |
          |                     ST_STATE                  |
          |                       seq_nr=c.seq_nr++       |
          |                       ack_nr=c.ack_nr         |
          |                       conn_id=c.send_conn_id  |
          | <------------------------------------------<  |
          | c.state = CS_CONNECTED                        |
          | c.ack_nr = pkt.seq_nr                         |
          |                                               |
          |                                               |
          |                                               |
          | ST_DATA                                       |
          |   seq_nr=c.seq_nr++                           |
          |   ack_nr=c.ack_nr                             |
          |   conn_id=c.conn_id_send                      |
          | >-------------------------------------------> |
          |                        c.ack_nr = pkt.seq_nr  |
          |                        c.state = CS_CONNECTED |
          |                                               |
          |                                               | connection established
     .. ..|.. .. .. .. .. .. .. .. .. .. .. .. .. .. .. ..|.. ..
          |                                               |
          |                     ST_DATA                   |
          |                       seq_nr=c.seq_nr++       |
          |                       ack_nr=c.ack_nr         |
          |                       conn_id=c.send_conn_id  |
          | <------------------------------------------<  |
          | c.ack_nr = pkt.seq_nr                         |
          |                                               |
          |                                               |
          V                                               V
```

Connections are identified by their conn\_id header. If the connection ID of a new connection collides with an existing connection, the connection attempt will fails, since the ST\_SYN packet will be unexpected in the existing stream, and ignored.

## packet loss

If the packet with sequence number (seq\_nr - cur\_window) has not been acked (this is the oldest packet in the send buffer, and the next one expected to be acked), but 3 or more packets have been acked past it (through Selective ACK), the packet is assumed to have been lost. Similarly, when receiving 3 duplicate acks, ack\_nr + 1 is assumed to have been lost (if a packet with that sequence number has been sent).

This is applied to selective acks as well. Each packet that is acked in the selective ack message counts as one duplicate ack, which, if it 3 or more, should trigger a re-send of packets that had at least 3 packets acked after them.

When a packet is lost, the max\_window is multiplied by 0.5 to mimic TCP.

## timeouts

Every packet that is ACKed, either by falling in the range (last\_ack\_nr, ack\_nr\] or by explicitly being acked by a Selective ACK message, should be used to update an rtt (round trip time) and rtt\_var (rtt variance) measurement. last\_ack\_nr here is the last ack\_nr received on the socket before the current packet, and ack\_nr is the field in the currently received packet.

The rtt and rtt\_var is only updated for packets that were sent only once. This avoids problems with figuring out which packet was acked, the first or the second one.

rtt and rtt\_var are calculated by the following formula, every time a packet is ACKed:

```
delta = rtt - packet_rtt
rtt_var += (abs(delta) - rtt_var) / 4;
rtt += (packet_rtt - rtt) / 8;
```

The default timeout for packets associated with the socket is also updated every time rtt and rtt\_var is updated. It is set to:

```
timeout = max(rtt + rtt_var * 4, 500);
```

Where timeout is specified in milliseconds. i.e. the minimum timeout for a packet is 1/2 second.

Every time a socket sends or receives a packet, it updates its timeout counter. If no packet has arrived within timeout number of milliseconds from the last timeout counter reset, the socket triggers a timeout. It will set its packet\_size and max\_window to the smallest packet size (150 bytes). This allows it to send one more packet, and this is how the socket gets started again if the window size goes down to zero.

The initial timeout is set to 1000 milliseconds, and later updated according to the formula above. For every packet consecutive subsequent packet that times out, the timeout is doubled.

## packet sizes

In order to have as little impact as possible on slow congested links, uTP adjusts its packet size down to as small as 150 bytes per packet. Using packets that small has the benefit of not clogging a slow up-link, with long serialization delay. The cost of using packets that small is that the overhead from the packet headers become significant. At high rates, large packet sizes are used, at slow rates, small packet sizes are used.

## congestion control

The overall goal of the uTP congestion control is to use one way buffer delay as the main congestion measurement, as well as packet loss, like TCP. The point is to avoid running with full send buffers whenever data is being sent. This is specifically a problem for DSL/Cable modems, where the send buffer in the modem often has room for multiple seconds worth of data. The ideal buffer utilization for uTP (or any background traffic protocol) is to run at 0 bytes buffer utilization. i.e. any other traffic can at any time send without being obstructed by background traffic clogging up the send buffer. In practice, the uTP target delay is set to 100 ms. Each socket aims to never see more than 100 ms delay on the send link. If it does, it will throttle back.

This effectively makes uTP yield to any TCP traffic.

This is achieved by including a high resolution timestamp in every packet that's sent over uTP, and the receiving end calculates the difference between its own high resolution timer and the timestamp in the packet it received. This difference is then fed back to the original sender of the packet (timestamp\_difference\_microseconds). This value is not meaningful as an absolute value. The clocks in the machines are most likely not synchronized, especially not down to microsecond resolution, and the time the packet is in transit is also included in the difference of these timestamps. However, the value is useful in comparison to previous values.

Each socket keeps a sliding minimum of the lowest value for the last two minutes. This value is called _base\_delay_, and is used as a baseline, the minimum delay between the hosts. When subtracting the base\_delay from the timestamp difference in each packet you get a measurement of the current buffering delay on the socket. This measurement is called _our\_delay_. It has a lot of noise it it, but is used as the driver to determine whether to increase or decrease the send window (which controls the send rate).

The _CCONTROL\_TARGET_ is the buffering delay that the uTP accepts on the up-link. Currently the delay target is set to 100 ms. _off\_target_ is how far the actual measured delay is from the target delay (calculated from CCONTROL\_TARGET - our\_delay).

The window size in the socket structure specifies the number of bytes we may have in flight (not acked) in total, on the connection. The send rate is directly correlated to this window size. The more bytes in flight, the faster send rate. In the code, the window size is called max\_window. Its size is controlled, roughly, by the following expression:

```
delay_factor = off_target / CCONTROL_TARGET;
window_factor = outstanding_packet / max_window;
scaled_gain = MAX_CWND_INCREASE_PACKETS_PER_RTT * delay_factor * window_factor;
```

Where the first factor scales the _off\_target_ to units of target delays.

The scaled\_gain is then added to the max\_window:

```
max_window += scaled_gain;
```

This will make the window smaller if off\_target is greater than 0 and grow the window if off target is less than 0.

If max\_window becomes less than 0, it is set to 0. A window size of zero means that the socket may not send any packets. In this state, the socket will trigger a timeout and force the window size to one packet size, and send one packet. See the section on timeouts for more information.
